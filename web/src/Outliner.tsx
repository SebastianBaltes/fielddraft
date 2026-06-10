/**
 * Feature outliner — flat list of features with selection + per-row op
 * switching. Mirrors Onshape's part-studio feature list, scoped down to
 * a flat spine for M2.
 */

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { type FeatureSummary } from "./kernel";
import type { FeatureFolder, KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { useMediaQuery, MOBILE_QUERY } from "./useMediaQuery";
import { FeatureDialog, FEATURE_TYPE_LABELS, FEATURE_TYPE_META } from "./widgets/FeatureDialog";
import {
  buildDefaultDefinition,
  type FeatureSchema,
} from "./widgets/schema";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import type { SketchDiagnostics } from "./solver/types";
import {
  buildDraftSketchState,
  idsOfSubCall,
  constraintRefIds,
  summarizeDiagnostics,
} from "./sketchSession";
import { getActive as getActivePicker } from "./queryPickerBridge";
import { bodyFeatureIds, resolveSelectionLinks } from "./selectionLinks";
import { formatQueryDisplay } from "./queryLabels";
import { ElementInfo } from "./ElementInfo";

/** A FieldSkript instance reports its `kind` as the script's id (e.g.
 *  `script_1`), which the static feature-type tables don't know about. The
 *  display name + icon live in the script record, surfaced via
 *  `state.scriptTypes` (same source the Toolbar buttons use). */
type ScriptType = { id: string; name: string; icon: string };

function scriptTypeFor(
  kind: string,
  scriptTypes?: readonly ScriptType[],
): ScriptType | undefined {
  return scriptTypes?.find((t) => t.id === kind);
}

export function kindGlyph(kind: string, scriptTypes?: readonly ScriptType[]): string {
  const script = scriptTypeFor(kind, scriptTypes);
  if (script) return script.icon;
  return FEATURE_TYPE_META[kind]?.glyph ?? "?";
}


function formatPlaneQuery(query: string | null): string {
  return query ? formatQueryDisplay(query) : "—";
}

export function prettifyFeatureName(
  name: string,
  kind: string,
  scriptTypes?: readonly ScriptType[],
): string {
  const prettyKind = scriptTypeFor(kind, scriptTypes)?.name ?? FEATURE_TYPE_LABELS[kind];
  if (!prettyKind) return name;
  if (name === kind) return prettyKind;
  const suffix = name.startsWith(kind + " ") ? name.slice(kind.length) : null;
  if (suffix) return prettyKind + suffix;
  return name;
}

const SKETCH_SUB_LABEL: Record<string, string> = {
  sketch_point: "Point",
  sketch_line: "Line",
  sketch_rectangle: "Rect",
  sketch_circle: "Circle",
  sketch_polygon: "Polygon",
  sketch_arc: "Arc",
  sketch_mirror: "Mirror",
  sketch_trim: "Trim",
  sketch_offset: "Offset",
  sketch_constraint_horizontal: "Horizontal",
  sketch_constraint_vertical: "Vertical",
  sketch_constraint_coincident: "Coincident",
  sketch_constraint_distance: "Distance",
  sketch_constraint_angle: "Angle",
  sketch_constraint_equal: "Equal",
  sketch_constraint_parallel: "Parallel",
  sketch_constraint_perpendicular: "Perpendicular",
  sketch_constraint_tangent: "Tangent",
  sketch_constraint_midpoint: "Midpoint",
  sketch_constraint_symmetric: "Symmetric",
  sketch_constraint_radius: "Radius",
  sketch_constraint_diameter: "Diameter",
};

interface OutlinerProps {
  store: KernelStore;
  state: StoreState;
  /** Mobile drawer: whether the off-canvas panel is slid in. Ignored on
   *  desktop, where the outliner is always docked. */
  mobileNavOpen?: boolean;
}

/** Outliner-splitter drag constraints. Width is clamped to these
 *  bounds on every drag tick and on localStorage rehydrate. */
const OUTLINER_MIN_WIDTH = 220; // px
const OUTLINER_MAX_WIDTH = 640; // px
const OUTLINER_WIDTH_STORAGE_KEY = "fielddraft.outlinerWidth";

function clampOutlinerWidth(px: number): number {
  if (!Number.isFinite(px)) return OUTLINER_MIN_WIDTH;
  return Math.max(OUTLINER_MIN_WIDTH, Math.min(OUTLINER_MAX_WIDTH, px));
}

function loadOutlinerWidth(defaultRem: number): number {
  try {
    const raw = localStorage.getItem(OUTLINER_WIDTH_STORAGE_KEY);
    if (raw != null) {
      const parsed = parseFloat(raw);
      if (Number.isFinite(parsed)) return clampOutlinerWidth(parsed);
    }
  } catch {
    // localStorage blocked — fall through to default.
  }
  // Convert rem → px using the document root font size.
  const rootFontSize =
    typeof window !== "undefined"
      ? parseFloat(getComputedStyle(document.documentElement).fontSize) || 16
      : 16;
  return clampOutlinerWidth(defaultRem * rootFontSize);
}

export function Outliner({ store, state, mobileNavOpen = false }: OutlinerProps) {
  // Below the phone breakpoint the outliner is an off-canvas drawer whose
  // width comes from CSS, so the splitter-driven inline width is dropped.
  const isMobile = useMediaQuery(MOBILE_QUERY);
  // K6a: memoised Outer API façade over the store's kernel. The
  // `createFdApi` return value is a tiny object of method refs,
  // so re-creating it per render is cheap — but memoising keeps
  // React's reference identity stable for downstream hooks.
  const fd = useMemo(() => createFdApi(store.kernel), [store]);

  // (b) K7.22 UX-fix: resizable outliner via splitter/gutter on the
  //     left edge. Width persists in localStorage so it survives
  //     reloads.
  const [outlinerWidth, setOutlinerWidth] = useState<number>(() =>
    loadOutlinerWidth(19),
  );
  const dragStateRef = useRef<{ startX: number; startWidth: number } | null>(
    null,
  );

  useEffect(() => {
    try {
      localStorage.setItem(OUTLINER_WIDTH_STORAGE_KEY, String(outlinerWidth));
    } catch {
      // ignore
    }
  }, [outlinerWidth]);

  const handleSplitterPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    (e.currentTarget as HTMLDivElement).setPointerCapture(e.pointerId);
    dragStateRef.current = { startX: e.clientX, startWidth: outlinerWidth };
  };
  const handleSplitterPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragStateRef.current;
    if (!drag) return;
    // Splitter is on the LEFT edge of the outliner; dragging left
    // widens the panel, so the delta is negated.
    const delta = drag.startX - e.clientX;
    setOutlinerWidth(clampOutlinerWidth(drag.startWidth + delta));
  };
  const handleSplitterPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    dragStateRef.current = null;
    try {
      (e.currentTarget as HTMLDivElement).releasePointerCapture(e.pointerId);
    } catch {
      // ignore
    }
  };
  const handleSplitterDoubleClick = () => {
    // Double-click restores the token default.
    setOutlinerWidth(loadOutlinerWidth(19));
  };

  // K7.5: the FeatureDialog's working copy mirrors the
  // currently-selected feature's real params. Whenever the
  // selection changes we reload the draft from the kernel,
  // and every dialog onChange commits the new blob back via
  // store.updateFeatureParams. Keyed on the feature id so
  // switching between two features of the same kind picks up
  // each one's own values, not a shared default.
  const [dialogDraft, setDialogDraft] = useState<
    Record<string, unknown>
  >({});
  // Element/body selection → related outliner rows (ticket r36f): the
  // features owning the selected B-Rep element (or contributing to the
  // highlighted body) render with the GUI selection background, so picking
  // geometry in the viewport always shows WHERE it came from in the tree.
  const relatedFeatureIds = useMemo<ReadonlySet<number>>(() => {
    if (!store) return new Set<number>();
    if (state.selectedElements.length > 0)
      return resolveSelectionLinks(store.kernel, state.selectedElements).featureIds;
    if (state.highlightedBodyIndex !== null)
      return bodyFeatureIds(store.kernel, state.highlightedBodyIndex);
    return new Set<number>();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, state.selectedElements, state.highlightedBodyIndex, state.features]);

  const selectedFeature = useMemo<FeatureSummary | undefined>(
    () =>
      state.selection.size === 1
        ? state.features.find((f) => state.selection.has(f.id))
        : undefined,
    [state.selection, state.features],
  );
  const selectedSchema = useMemo<FeatureSchema | null>(() => {
    if (!selectedFeature) return null;
    try {
      const schema = fd.schemaForType(selectedFeature.kind);
      // A committed sketch edits its PLANE from the detail panel (ticket
      // yx6a, Onshape's "Sketch plane" property); the sub_calls blob is
      // owned by the sketch session and would render as a giant JSON
      // textarea, so it is filtered out of the dialog.
      if (selectedFeature.kind === "sketch") {
        return { ...schema, params: schema.params.filter((p) => p.key === "plane") };
      }
      return schema;
    } catch {
      // The kernel may report a kind name the schema lookup
      // doesn't know about (future feature types, stale caches).
      // Silently skip the dialog in that case — the rest of the
      // outliner still functions.
      return null;
    }
  }, [fd, selectedFeature]);

  // Reload the draft from the kernel whenever the selected
  // feature id changes. The `draftKey` is the feature id plus
  // the schema type so a delete+re-add of the same id still
  // reinitialises cleanly.
  const draftKey =
    selectedFeature && selectedSchema
      ? `${selectedFeature.id}:${selectedSchema.type}`
      : null;
  const [lastDraftKey, setLastDraftKey] = useState<string | null>(null);
  const [dialogOpenUndoDepth, setDialogOpenUndoDepth] = useState(0);
  if (draftKey !== lastDraftKey) {
    setLastDraftKey(draftKey);
    if (selectedFeature && selectedSchema) {
      setDialogOpenUndoDepth(state.undoDepth);
      try {
        setDialogDraft(store.getFeatureParams(selectedFeature.id));
      } catch {
        setDialogDraft(buildDefaultDefinition(selectedSchema));
      }
    } else {
      setDialogDraft({});
    }
  }

  // Resync dialogDraft when the store's params change externally
  // (e.g. JS API call from DevTools). The features array reference
  // only changes on commit(), so this check fires exactly when the
  // kernel mutated — not on selection changes or mesh updates.
  const prevFeaturesRef = useRef(state.features);
  if (
    state.features !== prevFeaturesRef.current &&
    draftKey === lastDraftKey &&
    selectedFeature &&
    selectedSchema
  ) {
    try {
      const storeParams = store.getFeatureParams(selectedFeature.id);
      if (JSON.stringify(storeParams) !== JSON.stringify(dialogDraft)) {
        setDialogDraft(storeParams);
      }
    } catch { /* ignore */ }
  }
  prevFeaturesRef.current = state.features;
  const handleDialogChange = (next: Record<string, unknown>) => {
    setDialogDraft(next);
    if (!selectedFeature) return;
    // A sketch's GEOMETRY is owned by the reentry session, but its PLANE
    // is an ordinary query param (ticket yx6a): write plane changes
    // through, preserving everything else (notably sub_calls, which the
    // filtered dialog never carries).
    if (selectedFeature.kind === "sketch") {
      let current: Record<string, unknown>;
      try {
        current = store.getFeatureParams(selectedFeature.id);
      } catch {
        return;
      }
      if (typeof next.plane === "string" && next.plane !== current.plane) {
        store.updateFeatureParams(selectedFeature.id, { ...current, plane: next.plane });
      }
      return;
    }
    // K8.12: skip the kernel round-trip while a formula field
    // is still being typed (just the `=` sigil with no body).
    // The dialog keeps the draft locally; commit fires on the
    // next onChange once the formula is non-empty.
    const hasIncompleteFormula = Object.values(next).some(
      (v) =>
        v != null &&
        typeof v === "object" &&
        "value" in (v as Record<string, unknown>) &&
        typeof (v as Record<string, unknown>).value === "string" &&
        ((v as Record<string, unknown>).value as string).trim() === "=",
    );
    if (hasIncompleteFormula) return;
    store.updateFeatureParams(selectedFeature.id, next);
  };

  const handleDialogOk = () => {
    store.clearSelection();
  };

  const handleDialogCancel = () => {
    const undosNeeded = state.undoDepth - dialogOpenUndoDepth;
    for (let i = 0; i < undosNeeded; i++) {
      if (!store.undo()) break;
    }
    store.clearSelection();
  };

  const handleRowClick = (e: MouseEvent, id: number) => {
    if (e.shiftKey) {
      store.selectRangeTo(id);
    } else if (e.ctrlKey || e.metaKey) {
      store.toggleSelect(id);
    } else {
      store.selectOnly(id);
    }
  };

  // K8.5: inline rename state. Holds the id of the row
  // currently in rename mode plus the working-copy string.
  // Null means no row is being renamed.
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState<string>("");

  // K8.10: working copies for the Variables panel's "add new"
  // row. Blank name + blank value until the user types.
  const [newVarName, setNewVarName] = useState<string>("");
  const [newVarValue, setNewVarValue] = useState<string>("");

  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    try {
      const raw = localStorage.getItem("fielddraft.outliner.collapsed");
      if (raw) return new Set(JSON.parse(raw));
    } catch { /* ignore */ }
    return new Set<string>();
  });
  const toggleSection = (id: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      try { localStorage.setItem("fielddraft.outliner.collapsed", JSON.stringify([...next])); } catch { /* ignore */ }
      return next;
    });
  };

  // K7.22 UX-fix (4): right-click context menu state.
  // `null` when the menu is closed; an object with the click
  // coordinates + target feature id when open. Dismissed on
  // scroll, outside-click, or Escape.
  const [contextMenu, setContextMenu] = useState<
    { x: number; y: number; featureId: number } | null
  >(null);
  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", close, true);
    };
  }, [contextMenu]);

  // K7.22 UX-fix (1): drag/drop reorder state.
  // `draggingId` is the feature id being dragged; `dropIndex` is
  // where it will land on drop (the index *in the features list*
  // where the dragged feature will be inserted). The UI paints a
  // small horizontal line above the target row while hovering.
  const [draggingId, setDraggingId] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const handleFeatureDragStart = (
    e: React.DragEvent<HTMLLIElement>,
    id: number,
  ) => {
    setDraggingId(id);
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("fielddraft/feature-id", String(id));
  };
  const handleFeatureDragOver = (
    e: React.DragEvent<HTMLLIElement>,
    targetIndex: number,
  ) => {
    if (draggingId === null) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const rect = (e.currentTarget as HTMLLIElement).getBoundingClientRect();
    const midY = rect.top + rect.height / 2;
    const insertAbove = e.clientY < midY;
    setDropIndex(insertAbove ? targetIndex : targetIndex + 1);
  };
  const handleFeatureDragLeave = () => {
    // Defer the clear to the next frame so a DragOver on the neighbouring row
    // (which fires continuously and re-sets dropIndex) doesn't flicker the
    // indicator while dragging across the row boundary.
    requestAnimationFrame(() => {
      setDropIndex(null);
    });
  };
  const handleFeatureDrop = (e: React.DragEvent<HTMLLIElement>) => {
    e.preventDefault();
    if (draggingId === null || dropIndex === null) {
      setDraggingId(null);
      setDropIndex(null);
      return;
    }
    const fromIdx = state.features.findIndex((f) => f.id === draggingId);
    let to = dropIndex;
    // When inserting below the source row, the array shrinks by
    // one during the move, so the target index is offset by one.
    if (fromIdx !== -1 && to > fromIdx) to -= 1;
    if (fromIdx !== -1 && to !== fromIdx) {
      store.reorderFeature(draggingId, to);
    }
    setDraggingId(null);
    setDropIndex(null);
  };
  const handleFeatureDragEnd = () => {
    setDraggingId(null);
    setDropIndex(null);
  };

  // ── rollback bar drag ──────────────────────────────────────
  const [draggingRollback, setDraggingRollback] = useState(false);
  const [rollbackDropIndex, _setRollbackDropIndex] = useState<number | null>(null);
  const rollbackDropIndexRef = useRef<number | null>(null);
  const setRollbackDropIndex = (v: number | null) => {
    rollbackDropIndexRef.current = v;
    _setRollbackDropIndex(v);
  };

  const handleRollbackDragStart = (e: React.DragEvent) => {
    setDraggingRollback(true);
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("fielddraft/rollback-bar", "1");
  };
  const handleRollbackDragOverRow = (
    e: React.DragEvent,
    targetIndex: number,
  ) => {
    if (!draggingRollback) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = "move";
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const midY = rect.top + rect.height / 2;
    setRollbackDropIndex(e.clientY < midY ? targetIndex : targetIndex + 1);
  };
  const handleRollbackDrop = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!e.dataTransfer.types.includes("fielddraft/rollback-bar")) return;
    const idx = rollbackDropIndexRef.current;
    if (idx !== null) {
      store.setRollbackIndex(idx);
    }
    setDraggingRollback(false);
    setRollbackDropIndex(null);
  };
  const handleRollbackDragEnd = () => {
    setDraggingRollback(false);
    setRollbackDropIndex(null);
  };

  const openContextMenu = (e: React.MouseEvent, featureId: number) => {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, featureId });
  };

  const handleAddVariable = () => {
    const trimmed = newVarName.trim();
    if (trimmed.length === 0) return;
    const value = Number(newVarValue);
    if (!Number.isFinite(value)) return;
    if (store.setVariable(trimmed, value)) {
      setNewVarName("");
      setNewVarValue("");
    }
  };

  const handleVariableValueChange = (name: string, text: string) => {
    const value = Number(text);
    if (Number.isFinite(value)) {
      store.setVariable(name, value);
    }
  };

  const beginRename = (feature: FeatureSummary) => {
    setRenamingId(feature.id);
    setRenameDraft(feature.name);
  };

  const commitRename = () => {
    if (renamingId === null) return;
    const next = renameDraft.trim();
    if (next.length > 0) {
      store.renameFeature(renamingId, next);
    }
    setRenamingId(null);
    setRenameDraft("");
  };

  const cancelRename = () => {
    setRenamingId(null);
    setRenameDraft("");
  };

  const handleRenameKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commitRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelRename();
    }
    // Swallow so global shortcuts (Delete, Ctrl-Z, …) don't
    // fire while typing.
    e.stopPropagation();
  };

  /** K6b.3b: double-click on a sketch row reopens it for editing.
   *  Onshape convention — single-click selects, double-click
   *  enters the feature's own edit mode. K8.5 reuses double-click
   *  on every non-sketch row for inline rename. */
  const handleRowDoubleClick = (feature: FeatureSummary) => {
    if (feature.kind === "sketch") {
      store.editSketch(feature.id);
      return;
    }
    beginRename(feature);
  };

  const handleDelete = (e: MouseEvent, id: number) => {
    e.stopPropagation();
    store.removeFeature(id);
  };

  // K6b.2: when a sketch session is active, the outliner leads with a
  // sketch-session panel (tool FeatureDialog + sub-spine list) so the
  // user never loses the feature list but sees the sketch workspace
  // prominently on top. Schema fetching for the selected sketch tool
  // happens through the same fd facade as top-level features.
  const sketchSession = state.sketchSession;
  const [sketchToolSchema, setSketchToolSchema] =
    useState<FeatureSchema | null>(null);
  useEffect(() => {
    const tool = sketchSession.selectedTool;
    if (!tool) {
      setSketchToolSchema(null);
      return;
    }
    try {
      setSketchToolSchema(fd.schemaForSketchToolType(tool));
    } catch {
      setSketchToolSchema(null);
    }
  }, [fd, sketchSession.selectedTool]);

  return (
    <aside
      className={"fd-outliner" + (mobileNavOpen ? " is-open" : "")}
      data-testid="fielddraft-outliner"
      style={isMobile ? undefined : { width: `${outlinerWidth}px`, flexBasis: `${outlinerWidth}px` }}
    >
      <div
        className="splitter"
        data-testid="outliner-splitter"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize outliner"
        title="Drag to resize outliner (double-click to reset)"
        onPointerDown={handleSplitterPointerDown}
        onPointerMove={handleSplitterPointerMove}
        onPointerUp={handleSplitterPointerUp}
        onPointerCancel={handleSplitterPointerUp}
        onDoubleClick={handleSplitterDoubleClick}
      />
      {state.selectedElement ? (
        <ElementInfo store={store} state={state} />
      ) : null}
      {sketchSession.active ? (
        <section
          className="sketch-session"
          data-testid="sketch-session-panel"
        >
          <div className="header">
            <span>Sketch</span>
            <span
              className="plane-query"
              data-testid="sketch-session-plane"
            >
              {formatPlaneQuery(sketchSession.planeQuery)}
            </span>
          </div>
          {sketchSession.selectedTool && sketchToolSchema ? (
            <>
              <FeatureDialog
                schema={sketchToolSchema}
                definition={sketchSession.toolDraft}
                onChange={(next) => store.setSketchToolDraft(next)}
              />
              <div className="actions">
                <button
                  type="button"
                  className="apply"
                  data-testid="sketch-tool-apply"
                  onClick={() => store.applySketchToolDraft()}
                >
                  Apply
                </button>
                <button
                  type="button"
                  className="clear"
                  data-testid="sketch-tool-clear"
                  onClick={() => store.selectSketchTool(null)}
                >
                  Clear
                </button>
              </div>
            </>
          ) : (
            <div className="empty" data-testid="sketch-session-empty">
              Pick a sketch tool in the toolbar.
            </div>
          )}
          {sketchSession.subCalls.length > 0
            ? (() => {
                const selection = new Set(sketchSession.selection);
                const geometry: number[] = [];
                const constraints: number[] = [];
                sketchSession.subCalls.forEach((call, i) => {
                  if (call.type.startsWith("sketch_constraint")) constraints.push(i);
                  else geometry.push(i);
                });

                // F20: which constraint rows the last solve flagged.
                const diag = summarizeDiagnostics(
                  sketchSession.subCalls,
                  sketchSession.diagnostics,
                );

                const renderRow = (i: number) => {
                  const call = sketchSession.subCalls[i];
                  const isConstraint = call.type.startsWith("sketch_constraint");
                  const isConstruction = Boolean(call.params.construction);
                  const refIds = isConstraint
                    ? constraintRefIds(sketchSession.subCalls, i)
                    : idsOfSubCall(sketchSession.subCalls, i);
                  const isActive =
                    refIds.length > 0 && refIds.every((id) => selection.has(id));
                  const selectRow = () =>
                    store.setSketchSelection(refIds);
                  return (
                    <li
                      key={i}
                      className={
                        "row" +
                        (isConstraint ? " is-constraint" : "") +
                        (isConstruction ? " is-construction" : "") +
                        (isActive ? " is-active" : "")
                      }
                      role="button"
                      tabIndex={0}
                      aria-pressed={isActive}
                      data-testid={`sketch-element-row-${i}`}
                      onClick={selectRow}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          selectRow();
                        }
                      }}
                    >
                      <span className="kind">
                        {kindGlyph(call.type)}{" "}
                        {SKETCH_SUB_LABEL[call.type] ?? call.type.replace("sketch_", "")}
                      </span>
                      {isConstraint && diag.conflicts.has(i) ? (
                        <span
                          className="badge is-conflict"
                          title="This constraint conflicts with the others — delete one of them to resolve the over-constrained sketch."
                          data-testid={`sketch-constraint-conflict-${i}`}
                        >
                          conflict
                        </span>
                      ) : isConstraint && diag.redundants.has(i) ? (
                        <span
                          className="badge is-redundant"
                          title="This constraint is redundant — it is already implied by the others and can be removed."
                          data-testid={`sketch-constraint-redundant-${i}`}
                        >
                          redundant
                        </span>
                      ) : null}
                      {!isConstraint ? (
                        <button
                          type="button"
                          className={"row-construction" + (isConstruction ? " is-active" : "")}
                          title={
                            isConstruction
                              ? "Construction geometry (reference only) — click to make it solid"
                              : "Make this construction geometry (reference only, not extruded)"
                          }
                          aria-pressed={isConstruction}
                          data-testid={`sketch-sub-row-construction-${i}`}
                          onClick={(e) => {
                            e.stopPropagation();
                            store.toggleSketchConstructionAt(i);
                          }}
                        >
                          ⊘
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="row-delete"
                        title="Delete"
                        data-testid={`sketch-sub-row-delete-${i}`}
                        onClick={(e) => {
                          e.stopPropagation();
                          store.removeSketchSubCall(i);
                        }}
                      >
                        ×
                      </button>
                    </li>
                  );
                };

                // Single list so the `.row` count contract stays
                // geometry + constraints; section headers are
                // non-`.row` list items that split the two groups.
                return (
                  <ul
                    className="sub-spine elements"
                    data-testid="sketch-sub-spine"
                  >
                    {geometry.length > 0 ? (
                      <li
                        className="elements-header"
                        data-testid="sketch-elements-geometry"
                      >
                        Geometry ({geometry.length})
                      </li>
                    ) : null}
                    {geometry.map(renderRow)}
                    {constraints.length > 0 ? (
                      <li
                        className="elements-header"
                        data-testid="sketch-elements-constraints"
                      >
                        Constraints ({constraints.length})
                      </li>
                    ) : null}
                    {diag.conflictCount > 0 || diag.redundantCount > 0 ? (
                      <li
                        className={
                          "diagnostics-summary" +
                          (diag.conflictCount > 0 ? " has-conflict" : "")
                        }
                        data-testid="sketch-diagnostics-summary"
                      >
                        ⚠ {diag.conflictCount} conflicting, {diag.redundantCount}{" "}
                        redundant
                      </li>
                    ) : null}
                    {constraints.map(renderRow)}
                  </ul>
                );
              })()
            : null}
          <ConstraintManagerPanel
            subCalls={sketchSession.subCalls}
            store={store}
            diagnostics={sketchSession.diagnostics}
          />
        </section>
      ) : null}
      {!sketchSession.active ? (
        <section
          className="variables-panel"
          data-testid="variables-panel"
        >
          <div className="header accordion-header" onClick={() => toggleSection("variables")} data-testid="variables-header">
            <span className="accordion-chevron">{collapsed.has("variables") ? "▸" : "▾"}</span>
            <span>Variables</span>
          </div>
          {!collapsed.has("variables") && (<>
          {state.variables.length === 0 ? (
            <div
              className="empty"
              data-testid="variables-empty"
            >
              No variables.
            </div>
          ) : (
            <ul className="list">
              {state.variables.map((v) => (
                <li
                  key={v.name}
                  className="row"
                  data-testid={`variable-row-${v.name}`}
                >
                  <span className="variable-name">{v.name}</span>
                  <input
                    className="variable-value"
                    type="number"
                    inputMode="decimal"
                    aria-label={`Value of ${v.name}`}
                    data-testid={`variable-value-${v.name}`}
                    defaultValue={v.value}
                    onBlur={(e) =>
                      handleVariableValueChange(v.name, e.currentTarget.value)
                    }
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        (e.currentTarget as HTMLInputElement).blur();
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="row-delete"
                    title="Remove variable"
                    data-testid={`variable-delete-${v.name}`}
                    onClick={() => store.removeVariable(v.name)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="variables-add">
            <input
              className="variable-name-input"
              placeholder="name"
              aria-label="New variable name"
              data-testid="variable-new-name"
              value={newVarName}
              onChange={(e) => setNewVarName(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  handleAddVariable();
                }
              }}
            />
            <input
              className="variable-value-input"
              type="number"
              inputMode="decimal"
              placeholder="value"
              aria-label="New variable value"
              data-testid="variable-new-value"
              value={newVarValue}
              onChange={(e) => setNewVarValue(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  handleAddVariable();
                }
              }}
            />
            <button
              type="button"
              className="apply"
              data-testid="variable-add"
              onClick={handleAddVariable}
            >
              +
            </button>
          </div>
          </>)}
        </section>
      ) : null}
      {!sketchSession.active && state.assembly.active && (
        <section className="assembly-panel" data-testid="assembly-panel">
          <div className="header accordion-header" onClick={() => toggleSection("assembly")} data-testid="assembly-header">
            <span className="accordion-chevron">{collapsed.has("assembly") ? "▸" : "▾"}</span>
            <span>Assembly</span>
            <button
              type="button"
              className="exit-btn"
              title="Exit assembly mode"
              data-testid="exit-assembly"
              onClick={() => store.exitAssembly()}
            >
              ✗
            </button>
          </div>
          {!collapsed.has("assembly") && (<>
          {state.assembly.document.parts.length === 0 ? (
            <div className="empty">No parts. Import a .fdraft file.</div>
          ) : (
            <ul className="list">
              {state.assembly.document.parts.map((part) => (
                <li
                  key={part.id}
                  className={"row" + (part.id === state.assembly.selectedPartId ? " is-selected" : "")}
                  data-testid={`assembly-part-${part.id}`}
                  onClick={() => store.selectAssemblyPart(part.id)}
                >
                  <span className="kind">▣</span>
                  <span className="name">{part.name}</span>
                  <span className="meta">{part.meshBodies.length} bodies</span>
                  <button
                    type="button"
                    className="row-delete"
                    title="Remove part"
                    onClick={(e) => { e.stopPropagation(); store.removePartFromAssembly(part.id); }}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}
          <input
            type="file"
            accept=".fdraft,.json"
            className="hidden-file-input"
            data-testid="assembly-import-input"
            onChange={(e) => {
              const file = e.currentTarget.files?.[0];
              if (!file) return;
              file.text().then((json) => {
                store.addPartToAssembly(json, file.name.replace(/\.(fdraft|json)$/, ""));
              });
              e.currentTarget.value = "";
            }}
          />
          <button
            type="button"
            className="tool"
            title="Add a part (.fdraft file)"
            data-testid="assembly-add-part"
            onClick={() => {
              document.querySelector<HTMLInputElement>('[data-testid="assembly-import-input"]')?.click();
            }}
          >
            + Add Part
          </button>

          {state.assembly.document.mates.length > 0 && (
            <>
              <div className="header" style={{ marginTop: "var(--space-3)" }}>
                <span>Mates</span>
              </div>
              <ul className="list">
                {state.assembly.document.mates.map((mate) => (
                  <li key={mate.id} className="row" data-testid={`mate-${mate.id}`}>
                    <span className="kind">
                      {mate.type === "fixed" ? "🔒" : mate.type === "ball" ? "⚽" : mate.type === "prismatic" ? "↕" : "🔄"}
                    </span>
                    <span className="name">{mate.name}</span>
                    {(mate.type === "revolute" || mate.type === "cylindrical") && (
                      <input
                        type="range"
                        min={mate.minAngle ?? -180}
                        max={mate.maxAngle ?? 180}
                        value={mate.angle ?? 0}
                        title={`${mate.angle ?? 0}°`}
                        className="mate-slider"
                        onChange={(e) => store.setMateAngle(mate.id, Number(e.target.value))}
                      />
                    )}
                    {(mate.type === "prismatic" || mate.type === "cylindrical") && (
                      <input
                        type="range"
                        min={-100}
                        max={100}
                        value={mate.offset ?? 0}
                        title={`${mate.offset ?? 0} mm`}
                        className="mate-slider"
                        onChange={(e) => store.setMateOffset(mate.id, Number(e.target.value))}
                      />
                    )}
                    <button
                      type="button"
                      className="row-delete"
                      title="Remove mate"
                      onClick={() => store.removeMate(mate.id)}
                    >
                      ×
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}

          {state.assembly.document.parts.length >= 2 && (
            <MateCreator parts={state.assembly.document.parts} store={store} />
          )}
          </>)}
        </section>
      )}
      <div className="header accordion-header" onClick={() => toggleSection("datums")} data-testid="datums-header">
        <span className="accordion-chevron">{collapsed.has("datums") ? "▸" : "▾"}</span>
        <span>Default geometry</span>
      </div>
      {!collapsed.has("datums") && (
        <ul className="list datum-list">
          {([
            { id: "origin", glyph: "⊙", label: "Origin" },
            { id: "top",    glyph: "┼", label: "Top" },
            { id: "front",  glyph: "┼", label: "Front" },
            { id: "right",  glyph: "┼", label: "Right" },
          ] as const).map((datum) => {
            const visible = !state.hiddenDatums.has(datum.id);
            const datumSelected = state.selectedDatum === datum.id;
            return (
              <li
                key={datum.id}
                className={"row datum-row" + (datumSelected ? " is-selected" : "")}
                data-testid={`datum-${datum.id}`}
                onClick={() => store.selectDatum(datumSelected ? null : datum.id)}
              >
                <span className="kind">{datum.glyph}</span>
                <span className="feature-name">{datum.label}</span>
                <button
                  type="button"
                  className={"row-datum-visible" + (visible ? " is-visible" : "")}
                  title={visible ? "Hide" : "Show"}
                  onClick={(e) => {
                    e.stopPropagation();
                    store.toggleDatumVisible(datum.id);
                  }}
                >
                  {visible ? "◉" : "◎"}
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <div className="header features-header accordion-header" onClick={() => toggleSection("features")} data-testid="features-header">
        <span className="accordion-chevron">{collapsed.has("features") ? "▸" : "▾"}</span>
        <span>Features</span>
        <button
          type="button"
          className="new-folder-btn"
          title="New folder (group features)"
          data-testid="new-folder"
          onClick={() => store.createFolder()}
        >
          <span className="folder-glyph" aria-hidden="true">📁</span>
          <span className="plus-glyph" aria-hidden="true">+</span>
        </button>
      </div>
      {!collapsed.has("features") && (state.features.length === 0 && state.folders.length === 0 ? (
        <div className="empty">No features yet.</div>
      ) : (
        <ul
          className="list"
          onDragOver={draggingRollback ? (e) => {
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            setRollbackDropIndex(state.features.length);
          } : undefined}
          onDrop={draggingRollback ? handleRollbackDrop : undefined}
        >
          {(() => {
            // Set of feature ids that live in some folder — used to
            // filter the ungrouped list at the bottom.
            const grouped = new Set<number>();
            for (const folder of state.folders) {
              for (const id of folder.memberIds) grouped.add(id);
            }

            // Render a single feature row. Hoisted from the JSX so
            // both the folder groups and the ungrouped list can
            // call it with the same args — keeps row styling,
            // drag/drop wiring and context-menu plumbing in a
            // single place.
            const renderFeatureRow = (
              feature: FeatureSummary,
              index: number,
            ) => {
              const selected = state.selection.has(feature.id);
              const related = relatedFeatureIds.has(feature.id);
              const isDragged = draggingId === feature.id;
              const isRolledBack = !!feature.rolled_back;
              const showRbDrop = draggingRollback && rollbackDropIndex === index;
              const showRbDropBelow = draggingRollback
                && rollbackDropIndex === state.features.length
                && index === state.features.length - 1;
              return (
                <li
                  key={feature.id}
                  className={
                    "row" +
                    (selected || related ? " is-selected" : "") +
                    (feature.suppressed ? " is-suppressed" : "") +
                    (isRolledBack ? " is-rolled-back" : "") +
                    (isDragged ? " is-dragging" : "") +
                    (dropIndex === index ? " drop-above" : "") +
                    (dropIndex === index + 1 &&
                    index === state.features.length - 1
                      ? " drop-below"
                      : "") +
                    (showRbDrop ? " rollback-drop-above" : "") +
                    (showRbDropBelow ? " rollback-drop-below" : "")
                  }
                  data-testid={`feature-row-${feature.id}`}
                  data-feature-id={feature.id}
                  data-feature-kind={feature.kind}
                  data-feature-suppressed={feature.suppressed ? "true" : "false"}
                  draggable={!isRolledBack}
                  onDragStart={isRolledBack ? undefined : (e) => handleFeatureDragStart(e, feature.id)}
                  onDragOver={(e) => {
                    if (draggingRollback) handleRollbackDragOverRow(e, index);
                    else handleFeatureDragOver(e, index);
                  }}
                  onDragLeave={draggingRollback ? undefined : handleFeatureDragLeave}
                  onDrop={(e) => {
                    if (e.dataTransfer.types.includes("fielddraft/rollback-bar")) handleRollbackDrop(e);
                    else handleFeatureDrop(e);
                  }}
                  onDragEnd={draggingRollback ? handleRollbackDragEnd : handleFeatureDragEnd}
                  onClick={isRolledBack ? undefined : (e) => handleRowClick(e, feature.id)}
                  onDoubleClick={isRolledBack ? undefined : () => handleRowDoubleClick(feature)}
                  onContextMenu={isRolledBack ? undefined : (e) => openContextMenu(e, feature.id)}
                >
                  <span className="kind">
                    {kindGlyph(feature.kind, state.scriptTypes)}
                  </span>
                  {renamingId === feature.id ? (
                    <input
                      className="feature-name-input"
                      data-testid={`feature-rename-${feature.id}`}
                      value={renameDraft}
                      autoFocus
                      onClick={(e) => e.stopPropagation()}
                      onChange={(e) => setRenameDraft(e.currentTarget.value)}
                      onKeyDown={handleRenameKey}
                      onBlur={commitRename}
                    />
                  ) : (
                    <span
                      className={"feature-name" + (selected ? " is-editable" : "")}
                      onClick={selected ? (e) => {
                        e.stopPropagation();
                        beginRename(feature);
                      } : undefined}
                    >
                      {prettifyFeatureName(feature.name, feature.kind, state.scriptTypes)}
                    </span>
                  )}
                  {feature.kind === "sketch" ? (
                    <button
                      type="button"
                      className={
                        "row-sketch-visible" +
                        (state.visibleSketches.includes(feature.id)
                          ? " is-visible"
                          : "")
                      }
                      title={
                        state.visibleSketches.includes(feature.id)
                          ? "Hide sketch in 3D viewport"
                          : "Show sketch in 3D viewport"
                      }
                      aria-label={
                        state.visibleSketches.includes(feature.id)
                          ? "Hide sketch in 3D viewport"
                          : "Show sketch in 3D viewport"
                      }
                      data-testid={`sketch-visible-${feature.id}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        store.toggleSketchVisible(feature.id);
                      }}
                      // Two rapid clicks on the eye icon fire a
                      // `dblclick` event that bubbles up to the row
                      // <li> and opens the sketch editor via
                      // `handleRowDoubleClick`. stopPropagation on
                      // the plain onClick isn't enough — the
                      // dblclick dispatches separately. Swallow it
                      // here so the button only ever toggles.
                      onDoubleClick={(e) => e.stopPropagation()}
                    >
                      {state.visibleSketches.includes(feature.id) ? "◉" : "◎"}
                    </button>
                  ) : null}
                  {/* K7.22 UX-fix (4): suppress toggle moved to the
                      right-click context menu; the row is quieter
                      without the per-row eye icon. */}
                  <button
                    type="button"
                    className="row-delete"
                    title="Delete (right-click for more options)"
                    onClick={(e) => handleDelete(e, feature.id)}
                  >
                    ×
                  </button>
                </li>
              );
            };

            const rbIdx = state.rollbackIndex;
            const ungrouped = state.features
              .map((feature, index) => ({ feature, index }))
              .filter(({ feature }) => !grouped.has(feature.id));

            const rollbackBar = (
              <li
                key="__rollback__"
                className={
                  "rollback-bar" +
                  (draggingRollback ? " is-dragging" : "")
                }
                draggable
                onDragStart={handleRollbackDragStart}
                onDragEnd={handleRollbackDragEnd}
                title="Rollback bar"
              />
            );

            return (
              <>
                {state.folders.map((folder) => (
                  <FolderGroup
                    key={folder.id}
                    folder={folder}
                    features={state.features}
                    renderFeatureRow={renderFeatureRow}
                    store={store}
                  />
                ))}
                {rbIdx === 0 && state.features.length > 0 && rollbackBar}
                {ungrouped.flatMap(({ feature, index }) => {
                  const row = renderFeatureRow(feature, index);
                  if (index + 1 === rbIdx && rbIdx > 0 && rbIdx < state.features.length) return [row, rollbackBar];
                  return [row];
                })}
                {rbIdx >= state.features.length && rollbackBar}
              </>
            );
          })()}
        </ul>
      ))}
      {!sketchSession.active && (
        <section className="detail-panel" data-testid="detail-panel">
          <div className="header accordion-header" onClick={() => toggleSection("detail")} data-testid="detail-header">
            <span className="accordion-chevron">{collapsed.has("detail") ? "▸" : "▾"}</span>
            <span>Detail</span>
            {selectedSchema && (
              <span className="detail-actions" onClick={(e) => e.stopPropagation()}>
                <button className="detail-ok" data-testid="detail-ok" onClick={handleDialogOk} title="Accept">✓</button>
                <button className="detail-cancel" data-testid="detail-cancel" onClick={handleDialogCancel} title="Cancel">✗</button>
              </span>
            )}
          </div>
          {!collapsed.has("detail") && (selectedSchema ? (
            <FeatureDialog
              schema={selectedSchema}
              definition={dialogDraft}
              onChange={handleDialogChange}
            />
          ) : (
            <div className="empty">No feature selected.</div>
          ))}
        </section>
      )}
      {!sketchSession.active && selectedFeature?.kind === "imported_mesh" ? (
        <div className="fd-mesh-mode-toggle">
          <label>
            <input
              type="checkbox"
              checked={!!selectedFeature.suppressed}
              onChange={() => store.toggleFeatureSuppressed(selectedFeature.id)}
            />
            Reference only (transparent overlay)
          </label>
        </div>
      ) : null}
      {contextMenu ? (
        <FeatureContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          featureId={contextMenu.featureId}
          store={store}
          onClose={() => setContextMenu(null)}
          onRename={() => {
            const f = state.features.find((ff) => ff.id === contextMenu.featureId);
            if (f) beginRename(f);
            setContextMenu(null);
          }}
        />
      ) : null}
      <PartTree store={store} state={state} collapsed={collapsed.has("parts")} onToggle={() => toggleSection("parts")} />
    </aside>
  );
}

/**
 * Part tree — lists the distinct solid bodies currently in the
 * document. The kernel partitions the feature spine into disjoint
 * bodies (K3 part 2) where every `Union` that overlaps an existing
 * body merges into it, so what the user sees here is one row per
 * physical solid, not per feature. Each body is named after its
 * seed feature, and clicking a body selects that seed in the
 * feature tree — a small but important affordance for anyone
 * editing a multi-body scene, where "which body does this feature
 * belong to" is otherwise invisible.
 */
function PartTree({ store, state, collapsed, onToggle }: { store: KernelStore; state: StoreState; collapsed: boolean; onToggle: () => void }) {
  // Source the part list straight from the kernel body list, not from
  // `meshedBodies`: DC meshing (and thus `meshedBodies`) only runs in mesh
  // render mode, so a body added while in an SDF mode would never show up as
  // a part. `bodyCount()`/`bodySeedFeatureId()` reflect the partitioned
  // bodies in every render mode. Triangle counts are only meaningful once a
  // mesh exists, so they default to 0 in SDF modes. Keyed on `state.features`
  // so it recomputes on every document edit (add/remove/move).
  const bodies = useMemo(() => {
    const k = store.kernel;
    const n = k.bodyCount();
    // Triangle counts live on the worker-meshed bodies (the main-thread
    // kernel keeps empty meshes in worker mode); look them up by body id and
    // fall back to 0 when no mesh exists yet (any SDF mode).
    const triById = new Map<number, number>(
      state.meshedBodies.map((b) => [b.id, Math.round(b.indices.length / 3)]),
    );
    const out: { id: number; seedId: number; triangleCount: number }[] = [];
    for (let i = 0; i < n; i++) {
      const id = k.bodyId(i);
      out.push({
        id,
        seedId: k.bodySeedFeatureId(i),
        triangleCount: triById.get(id) ?? 0,
      });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, state.features, state.meshedBodies]);
  const highlighted = state.highlightedBodyIndex;

  // The body owning the selected B-Rep element renders as selected too
  // (ticket r36f) — same GUI selection colour as an explicitly highlighted
  // body, so viewport picks always locate their part in the list.
  const relatedBodyIndex = useMemo<number | null>(() => {
    if (!store || state.selectedElements.length === 0) return null;
    return resolveSelectionLinks(store.kernel, state.selectedElements).bodyIndex;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, state.selectedElements, state.features]);

  const handlePartClick = (e: React.MouseEvent, index: number, seedFeature: FeatureSummary | undefined) => {
    e.stopPropagation();
    if (!seedFeature) return;
    const picker = getActivePicker();
    if (picker) {
      picker.onPick(`qCreatedBy(${seedFeature.id})`);
      return;
    }
    store.highlightBody(highlighted === index ? null : index);
  };

  return (
    <section className="part-tree" data-testid="part-tree">
      <div className="header accordion-header" onClick={onToggle} data-testid="parts-header">
        <span className="accordion-chevron">{collapsed ? "▸" : "▾"}</span>
        Parts ({bodies.length})
      </div>
      {!collapsed && (bodies.length === 0 ? (
        <div className="empty">No parts yet.</div>
      ) : <ul className="list">
        {bodies.map((body, index) => {
          const seedFeature = state.features.find((f) => f.id === body.seedId);
          const name = seedFeature
            ? prettifyFeatureName(seedFeature.name, seedFeature.kind, state.scriptTypes)
            : `Body ${index + 1}`;
          const kind = seedFeature?.kind ?? "body";
          const selected = highlighted === index || relatedBodyIndex === index;
          return (
            <li
              key={`${body.id}-${index}`}
              className={"row" + (selected ? " is-selected" : "")}
              data-testid={`part-row-${body.id}`}
              onClick={(e) => handlePartClick(e, index, seedFeature)}
              onDoubleClick={() => {
                if (seedFeature) store.selectOnly(seedFeature.id);
              }}
            >
              <span
                className="swatch"
                data-body-index={index}
                aria-hidden="true"
              />
              <span className="kind">{kindGlyph(kind, state.scriptTypes)}</span>
              <span className="name">{name}</span>
              {body.triangleCount > 0 && (
                <span className="triangle-count">{body.triangleCount}</span>
              )}
            </li>
          );
        })}
      </ul>)}
    </section>
  );
}

/**
 * One collapsible folder group in the Outliner feature list.
 *
 * Folders are a pure UI affordance from K7.22 UX-fix (2): they
 * don't exist in the kernel, they don't participate in the
 * parametric regeneration, and the feature spine stays linear
 * top-down regardless of how the user groups things. The folder
 * header provides:
 *   - a chevron that collapses/expands the member rows
 *   - the folder name (double-click to rename)
 *   - a drop zone that accepts a dragged feature and adds it to
 *     this folder's membership list
 *   - a small delete button that ungroups the members
 */
function FolderGroup({
  folder,
  features,
  renderFeatureRow,
  store,
}: {
  folder: FeatureFolder;
  features: readonly FeatureSummary[];
  renderFeatureRow: (feature: FeatureSummary, index: number) => React.ReactNode;
  store: KernelStore;
}) {
  const [dragOver, setDragOver] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [draftName, setDraftName] = useState(folder.name);

  // Sync the rename draft with the store when the folder is
  // renamed externally (undo, other tab, etc.).
  useEffect(() => {
    setDraftName(folder.name);
  }, [folder.name]);

  // Resolve the folder's member ids to actual FeatureSummary
  // objects, preserving document order (not insertion order) so
  // the folder's contents track the kernel's truth.
  const memberSet = new Set(folder.memberIds);
  const memberRows = features
    .map((feature, index) => ({ feature, index }))
    .filter(({ feature }) => memberSet.has(feature.id));

  return (
    <li
      className={
        "folder-group" +
        (folder.collapsed ? " is-collapsed" : "") +
        (dragOver ? " drag-over" : "")
      }
      data-testid={`folder-${folder.id}`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("fielddraft/feature-id")) {
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setDragOver(true);
        }
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        const raw = e.dataTransfer.getData("fielddraft/feature-id");
        const featureId = raw ? Number(raw) : NaN;
        if (Number.isFinite(featureId)) {
          store.addFeatureToFolder(featureId, folder.id);
        }
      }}
    >
      <div className="folder-header">
        <button
          type="button"
          className="chevron"
          title={folder.collapsed ? "Expand folder" : "Collapse folder"}
          data-testid={`folder-collapse-${folder.id}`}
          onClick={() => store.toggleFolderCollapsed(folder.id)}
        >
          {folder.collapsed ? "▸" : "▾"}
        </button>
        <span className="folder-icon" aria-hidden="true">
          📁
        </span>
        {renaming ? (
          <input
            className="folder-name-input"
            autoFocus
            value={draftName}
            onChange={(e) => setDraftName(e.currentTarget.value)}
            onBlur={() => {
              store.renameFolder(folder.id, draftName);
              setRenaming(false);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                (e.currentTarget as HTMLInputElement).blur();
              } else if (e.key === "Escape") {
                setDraftName(folder.name);
                setRenaming(false);
              }
            }}
          />
        ) : (
          <span
            className="folder-name"
            onDoubleClick={() => setRenaming(true)}
          >
            {folder.name}
          </span>
        )}
        <span className="folder-count">({memberRows.length})</span>
        <button
          type="button"
          className="folder-delete"
          title="Delete folder (features stay)"
          data-testid={`folder-delete-${folder.id}`}
          onClick={() => store.deleteFolder(folder.id)}
        >
          ×
        </button>
      </div>
      {!folder.collapsed && memberRows.length > 0 ? (
        <ul className="folder-members">
          {memberRows.map(({ feature, index }) => renderFeatureRow(feature, index))}
        </ul>
      ) : null}
    </li>
  );
}

/**
 * Right-click context menu for a feature row. Floats next to the
 * cursor with the standard feature operations (rename, duplicate,
 * suppress, move to folder, delete). The menu closes on
 * outside-click or Escape — both wired up by the owning
 * `Outliner` via a `useEffect` that mounts global listeners
 * while the menu is open.
 */
function FeatureContextMenu({
  x,
  y,
  featureId,
  store,
  onClose,
  onRename,
}: {
  x: number;
  y: number;
  featureId: number;
  store: KernelStore;
  onClose: () => void;
  onRename: () => void;
}) {
  // Clamp the menu inside the viewport so it never renders
  // offscreen. The global outside-click listener lives in the
  // parent Outliner effect.
  const clampedX = Math.min(x, window.innerWidth - 220);
  const clampedY = Math.min(y, window.innerHeight - 240);

  const item = (
    label: string,
    action: () => void,
    testid: string,
    danger = false,
  ) => (
    <button
      type="button"
      className={"menu-item" + (danger ? " is-danger" : "")}
      data-testid={testid}
      onClick={(e) => {
        e.stopPropagation();
        action();
        onClose();
      }}
    >
      {label}
    </button>
  );

  return (
    <div
      className="fd-context-menu"
      data-testid="feature-context-menu"
      style={{ left: `${clampedX}px`, top: `${clampedY}px` }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
    >
      {item("Rename", onRename, "context-rename")}
      {item(
        "Duplicate",
        () => store.duplicateFeature(featureId),
        "context-duplicate",
      )}
      {item(
        "Toggle suppress",
        () => store.toggleFeatureSuppressed(featureId),
        "context-suppress",
      )}
      {item("Move up", () => store.moveFeatureUp(featureId), "context-move-up")}
      {item(
        "Move down",
        () => store.moveFeatureDown(featureId),
        "context-move-down",
      )}
      <div className="menu-sep" aria-hidden="true" />
      {item(
        "Delete",
        () => store.removeFeature(featureId),
        "context-delete",
        true,
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  P1.1: Mate Creator                                                 */
/* ------------------------------------------------------------------ */

function MateCreator({
  parts,
  store,
}: {
  parts: ReadonlyArray<import("./assembly").AssemblyPartInstance>;
  store: KernelStore;
}) {
  const [expanded, setExpanded] = useState(false);
  const [mateType, setMateType] = useState<import("./assembly").MateType>("fixed");
  const [partAId, setPartAId] = useState(parts[0]?.id ?? "");
  const [connAId, setConnAId] = useState(0);
  const [partBId, setPartBId] = useState(parts[1]?.id ?? "");
  const [connBId, setConnBId] = useState(0);

  if (!expanded) {
    return (
      <button
        type="button"
        className="tool"
        style={{ marginTop: "var(--space-1)" }}
        data-testid="add-mate-btn"
        onClick={() => setExpanded(true)}
      >
        + Add Mate
      </button>
    );
  }

  const partA = parts.find((p) => p.id === partAId);
  const partB = parts.find((p) => p.id === partBId);

  return (
    <div className="mate-creator" data-testid="mate-creator">
      <div className="field">
        <label>Type</label>
        <select
          value={mateType}
          onChange={(e) => setMateType(e.target.value as import("./assembly").MateType)}
        >
          <option value="fixed">Fixed</option>
          <option value="revolute">Revolute</option>
          <option value="prismatic">Prismatic</option>
          <option value="cylindrical">Cylindrical</option>
          <option value="ball">Ball</option>
        </select>
      </div>
      <div className="field">
        <label>Part A</label>
        <select value={partAId} onChange={(e) => setPartAId(e.target.value)}>
          {parts.map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
        </select>
      </div>
      {partA && partA.mateConnectors.length > 0 && (
        <div className="field">
          <label>Connector A</label>
          <select
            value={connAId}
            onChange={(e) => setConnAId(Number(e.target.value))}
          >
            {partA.mateConnectors.map((c) => (
              <option key={c.featureId} value={c.featureId}>
                #{c.featureId} ({c.origin.map((v) => v.toFixed(0)).join(", ")})
              </option>
            ))}
          </select>
        </div>
      )}
      <div className="field">
        <label>Part B</label>
        <select value={partBId} onChange={(e) => setPartBId(e.target.value)}>
          {parts.map((p) => (
            <option key={p.id} value={p.id}>{p.name}</option>
          ))}
        </select>
      </div>
      {partB && partB.mateConnectors.length > 0 && (
        <div className="field">
          <label>Connector B</label>
          <select
            value={connBId}
            onChange={(e) => setConnBId(Number(e.target.value))}
          >
            {partB.mateConnectors.map((c) => (
              <option key={c.featureId} value={c.featureId}>
                #{c.featureId} ({c.origin.map((v) => v.toFixed(0)).join(", ")})
              </option>
            ))}
          </select>
        </div>
      )}
      <div className="field" style={{ display: "flex", gap: "var(--space-2)" }}>
        <button
          type="button"
          className="tool"
          data-testid="confirm-mate"
          disabled={partAId === partBId}
          onClick={() => {
            store.addMate(mateType, partAId, connAId, partBId, connBId);
            setExpanded(false);
          }}
        >
          Create
        </button>
        <button
          type="button"
          className="tool"
          onClick={() => setExpanded(false)}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  K7.21d: Constraint Manager Panel                                   */
/* ------------------------------------------------------------------ */

function ConstraintManagerPanel({
  subCalls,
  store,
  diagnostics,
}: {
  subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>;
  store: KernelStore;
  diagnostics: SketchDiagnostics;
}) {
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    index: number;
    isDriven: boolean;
  } | null>(null);

  useEffect(() => {
    if (!contextMenu) return;
    const close = () => setContextMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [contextMenu]);

  const constraintEntries = subCalls
    .map((call, index) => ({ call, index }))
    .filter(({ call }) => call.type.startsWith("sketch_constraint"));

  if (constraintEntries.length === 0) return null;

  let dof = 0;
  try {
    const skState = buildDraftSketchState(subCalls as Array<{ type: string; params: Record<string, unknown> }>);
    dof = displayDegreesOfFreedom(skState, diagnostics);
  } catch {
    // Malformed sub-calls (e.g. after undo) — fall back to 0 DoF.
  }
  // The authoritative solver DoF is never negative; over-constraint is
  // reported through conflicting/malformed constraints. The naive
  // fallback (before the first solve) can still go negative, which we
  // keep rendering as an "excess" hint.
  const dofOver =
    diagnostics.conflicting.length > 0 ||
    diagnostics.malformed.length > 0 ||
    dof < 0;
  const dofLabel =
    dofOver
      ? dof < 0
        ? `Over-constrained (${Math.abs(dof)} excess)`
        : "Over-constrained (conflict)"
      : dof > 0
        ? `Under-constrained (${dof} DoF)`
        : "Fully constrained";
  const dofClass = dofOver ? "dof-over" : dof > 0 ? "dof-under" : "dof-full";

  const counts: Record<string, number> = {};
  for (const { call } of constraintEntries) {
    const label = call.type.replace("sketch_constraint_", "");
    counts[label] = (counts[label] ?? 0) + 1;
  }

  return (
    <div className="constraint-manager" data-testid="constraint-manager">
      <div className="header">
        <span>Constraints</span>
        <span className={`dof-badge ${dofClass}`} data-testid="dof-badge">
          {dofLabel}
        </span>
      </div>

      <div className="constraint-summary" data-testid="constraint-summary">
        {Object.entries(counts).map(([k, v]) => (
          <span key={k} className="badge">
            {k} ×{v}
          </span>
        ))}
      </div>

      <ul className="constraint-list" data-testid="constraint-list">
        {constraintEntries.map(({ call, index }) => {
          // Distance, radius and diameter carry a millimetre value;
          // angle carries a degree value. The rest are non-dimensional.
          const isMmDim =
            call.type === "sketch_constraint_distance" ||
            call.type === "sketch_constraint_radius" ||
            call.type === "sketch_constraint_diameter";
          const isDimensional = isMmDim || call.type === "sketch_constraint_angle";
          const isDriven = isDimensional && Boolean(call.params.driven);
          const dimUnit = isMmDim ? "mm" : "°";
          const dimValue = isDimensional
            ? Number(isMmDim ? call.params.value_mm : call.params.value_deg)
            : NaN;

          return (
            <li
              key={index}
              className={
                "constraint-row" +
                (isDriven ? " is-driven" : "") +
                (isDimensional ? " is-dimensional" : "")
              }
              data-testid={`constraint-row-${index}`}
              onContextMenu={(e) => {
                if (!isDimensional) return;
                e.preventDefault();
                setContextMenu({
                  x: e.clientX,
                  y: e.clientY,
                  index,
                  isDriven,
                });
              }}
            >
              <span className="glyph">
                {kindGlyph(call.type)}
              </span>
              <span className="name">
                {SKETCH_SUB_LABEL[call.type] ?? call.type.replace("sketch_constraint_", "")}
              </span>
              {isDimensional && (
                <span className="value">
                  <input
                    key={dimValue}
                    className="dim-input"
                    type="number"
                    inputMode="decimal"
                    aria-label={`Dimension value (${dimUnit})`}
                    data-testid={`constraint-value-${index}`}
                    defaultValue={Number.isFinite(dimValue) ? dimValue : ""}
                    disabled={isDriven}
                    title={
                      isDriven
                        ? "Driven dimension (read-only)"
                        : "Edit to drive the geometry"
                    }
                    onBlur={(e) => {
                      const n = Number(e.currentTarget.value);
                      if (Number.isFinite(n)) {
                        store.setSketchConstraintValue(index, n);
                      }
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        (e.currentTarget as HTMLInputElement).blur();
                      }
                    }}
                  />
                  <span className="unit">{dimUnit}</span>
                </span>
              )}
              {isDimensional && (
                <span
                  className={"driven-tag" + (isDriven ? " is-driven" : "")}
                  title={isDriven ? "Driven (informational)" : "Driving (controls geometry)"}
                >
                  {isDriven ? "D" : "⏚"}
                </span>
              )}
              <button
                type="button"
                className="row-delete"
                title="Delete constraint"
                data-testid={`constraint-delete-${index}`}
                onClick={() => store.removeSketchSubCall(index)}
              >
                ×
              </button>
            </li>
          );
        })}
      </ul>

      {contextMenu && (
        <div
          className="constraint-context-menu"
          data-testid="constraint-context-menu"
          style={{
            position: "fixed",
            left: contextMenu.x,
            top: contextMenu.y,
          }}
        >
          <button
            type="button"
            data-testid="toggle-driven"
            onClick={() => {
              store.toggleConstraintDriven(contextMenu.index);
              setContextMenu(null);
            }}
          >
            {contextMenu.isDriven ? "Change to Driving" : "Change to Driven"}
          </button>
        </div>
      )}
    </div>
  );
}
