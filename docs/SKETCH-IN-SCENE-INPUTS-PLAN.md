# Sketch In-Scene Inputs — Integration Plan

Ticket pqoe asks to move the sketch editor's tool-parameter fields out of the
top toolbar and into the scene, next to where they act. The first batch —
**fillet radius**, **chamfer distance**, **offset distance**, plus the earlier
**text** input — is already in-scene (see `web/src/SketchCanvas2d.tsx`, the
`.fd-sketch-tool-param` and `.fd-sketch-text-editor` overlays). This document
plans the remaining fields and the two deeper "click the result to re-edit it"
asks.

Two reusable patterns exist; every item below reuses one of them:

- **Pinned tool-param overlay** (`.fd-sketch-tool-param`) — a small DOM input
  pinned to a fixed canvas corner, shown only while the owning tool is active,
  bound straight to a store setter. Best for a parameter that applies to the
  whole tool gesture rather than one location (fillet/chamfer/offset use this).
- **Anchored editor overlay** (`.fd-sketch-text-editor`, the line-length
  `editor`) — a DOM input positioned at a sketch-plane `(u, v)` anchor in
  client px so it tracks the geometry, opened by a click and committed on
  Enter/blur. Best for editing one specific entity in place.

The store state/setters named below already exist (`store.ts`); none of the
already-shipped work needed new store logic, and most of the items below need
none either.

## 1. N-gon (regular polygon) sides count

The regular-polygon tool (`sketch_regular_polygon`) drags a radius from a
centre; its **side count** and **inscribed/circumscribed** flag come from the
tool draft (`session.toolDraft.sides` / `.inscribed`, read in
`buildSketchHudUv`), not from a dedicated toolbar field. There is no
`sketch-pattern-count` reuse here — that testid belongs to the Pattern tool
(item 2). Today the side count is only adjustable after the fact via the
FeatureDialog.

- **Anchor:** pinned tool-param overlay, same corner as fillet/chamfer/offset.
  Render it whenever `selectedTool === "sketch_regular_polygon"`. Show two
  controls: a number input for sides (min 3) and a small inscribed/
  circumscribed toggle.
- **Trigger:** tool active (no click needed — it must be set before the drag).
- **Store binding:** the value lives in the tool draft, so bind through
  `store.setSketchToolDraft({ sides, inscribed })` (the same call the gesture
  already uses). Add a `ToolParamField`-style descriptor for the polygon tool
  in `toolParamField`, but since it has two controls it is cleaner to special-
  case it in the JSX rather than force it through the single-input descriptor.
- **Steps:** (a) extend the in-scene overlay block in `SketchCanvas2d.tsx` to
  branch on `sketch_regular_polygon` and render sides + toggle; (b) read the
  defaults from `session.toolDraft`; (c) write back via `setSketchToolDraft`;
  (d) give the inputs stable testids (e.g. `sketch-ngon-sides`,
  `sketch-ngon-inscribed`) and add a unit/E2E check; (e) no store change.

## 2. Pattern count / spacing / angle

Still in the toolbar: `sketch-pattern-count` (`sketchPatternCount`),
`sketch-pattern-spacing` (`sketchPatternSpacing`), `sketch-pattern-angle`
(`sketchPatternAngle`), driving `store.setSketchPatternCount/Spacing/Angle` and
the `Linear X` / `Linear Y` / `Circular` action buttons.

These three are not a single param and they sit next to action buttons, so a
one-input pinned overlay does not fit cleanly. Two options:

- **Preferred:** a small pinned **pattern panel** overlay (a sibling of
  `.fd-sketch-tool-param`, e.g. `.fd-sketch-pattern-panel`) shown while a
  dedicated `sketch_pattern` tool is active. It holds count + spacing + angle
  inputs and the three apply buttons. This requires introducing a
  `sketch_pattern` tool button (currently Pattern has no tool mode — it is
  pure toolbar buttons acting on the current selection). The tool's "active"
  state is just "show the panel"; the apply buttons keep calling
  `store.linearPatternSketch("x"|"y")` / `circularPatternSketch()`.
- **Lighter:** keep the apply buttons in the toolbar but move only the three
  numeric inputs into the pinned overlay, gated on a non-empty selection. Less
  consistent (controls split across toolbar and scene) but no new tool.
- **Anchor:** pinned overlay corner (offset below the tool-param slot so they
  do not overlap if both ever show).
- **Trigger:** selection present (lighter) or `sketch_pattern` tool active
  (preferred).
- **Store binding:** existing setters and existing pattern actions — no store
  change.
- **Steps:** (a) decide tool-vs-selection gating; (b) add the overlay panel +
  CSS; (c) move the three inputs and (preferred) the three buttons; (d) keep
  the existing testids; (e) remove the toolbar block.

## 3. Rotate angle

Still in the toolbar: `sketch-rotate-angle` (`sketchRotateAngle`) →
`store.setSketchRotateAngle`, with a `Rotate` button calling
`store.rotateSketchSelection(angle, false)` about the origin.

- **Anchor:** pinned overlay corner, shown while a selection exists (Rotate has
  no tool mode, like Pattern). A single angle input plus the apply button —
  this is the closest analogue to the fillet param, just with an action button.
- **Trigger:** non-empty selection (could be folded into the same pattern panel
  from item 2 since both act on the selection — a single "transform" panel
  holding pattern + rotate would be the cleanest end state).
- **Store binding:** `store.setSketchRotateAngle` + `rotateSketchSelection` —
  no store change.
- **Steps:** (a) render an angle input + Rotate button in the pinned panel when
  a selection exists; (b) keep `sketch-rotate-angle` / `sketch-rotate-apply`
  testids; (c) remove the toolbar block.

## 4. Post-hoc per-entity fillet/chamfer radius (parametric corners)

The ticket's literal ask: "click the fillet rounding to edit its radius
in-scene," the way a line's length is click-editable. **This is not possible
with the current data model.** `store.filletSketchAt` (and `chamferSketchAt`)
call `filletLines` / `chamferLines`, which return baked geometry —
`filletSketchAt` trims the two source lines in place and **appends a plain
`sketch_arc`** (`subCallOfCurve(fillet.arc, …)`), carrying only a construction
flag. There is no stored radius and no reference to the two lines that produced
the arc, so a clicked arc cannot be re-solved back into "a fillet of radius R
between line A and line B." Chamfer is the same with a straight connector line.

To make corners re-editable they must become **parametric sub-calls**:

- **Data-model change:** introduce a `sketch_fillet` sub-call (and
  `sketch_chamfer`) whose params are `{ line_a, line_b, radius }` (entity/sub-
  call refs + the scalar). At solve/derive time the draft builder re-runs
  `filletLines` from the current positions of the two referenced lines to
  produce the displayed arc and the trimmed line ends — the arc becomes a
  *derived* curve, not a stored one. This mirrors how `sketch_arc` etc. are
  already evaluated from params in `buildDraftSketchState`.
- **Anchor:** anchored editor overlay (the text/line-length pattern) opened at
  the derived arc's midpoint `(u, v)` in client px.
- **Trigger:** in Select mode, a hit-test click on a derived fillet/chamfer arc
  opens the radius editor (extend `hitTestSketchEntity` so a derived corner
  reports its owning `sketch_fillet` sub-call index).
- **Store binding:** a new `store.setSketchFilletRadius(index, radius)`-style
  per-entity setter that rewrites the sub-call's `radius` param and re-derives
  (parallels `store.setSketchLineLength(index, len)`).
- **Steps (rough):** (a) add the `sketch_fillet` / `sketch_chamfer` sub-call
  types and their evaluation in `buildDraftSketchState` (re-derive arc +
  trimmed lines from refs); (b) change `filletSketchAt` to emit the parametric
  sub-call instead of baking lines+arc; (c) teach the hit-test + grips to
  surface the derived corner and map a click to its sub-call; (d) add the
  anchored radius editor in `SketchCanvas2d.tsx` + the per-entity setter; (e)
  migrate/adjust the existing fillet/chamfer E2E specs (they assert a baked
  `sketch_arc` / connector line appears — under the parametric model the
  *derived* geometry must be asserted instead, or the assertion shifts to the
  new sub-call type). This is the largest item; it touches the sketch data
  model, so it should land as its own change with its own tests.

## 5. Re-edit committed TEXT in-scene

A committed text is exploded to per-glyph `sketch_path` contours
(`store.placeSketchText` → one `sketch_path` per glyph, "carrying its outer
ring + holes"). The original string, height, and font are **not recoverable**
from the contours, so a clicked glyph cannot reopen the text editor with its
text.

Same shape of fix as item 4 — make text **parametric**:

- **Data-model change:** introduce a `sketch_text` sub-call storing
  `{ origin, value, height, fontId }`. The glyph `sketch_path` contours become
  *derived* from this sub-call at evaluation time (run the existing glyph-
  tessellation in the draft builder) rather than baked once at placement. The
  string/height/font then survive in the sub-call.
- **Anchor:** the existing `.fd-sketch-text-editor` anchored overlay, reopened
  at the stored `origin` projected to client px.
- **Trigger:** in Select mode, a hit-test click on derived text geometry maps
  to its owning `sketch_text` sub-call and reopens the editor pre-filled with
  the stored value/height/font.
- **Store binding:** reuse `setSketchTextValue/Height/FontId`, but committing
  must update the *existing* sub-call in place (a per-index update) rather than
  appending a new placement.
- **Steps (rough):** (a) add the `sketch_text` sub-call + its derivation in
  `buildDraftSketchState` (string/height/font → glyph contours); (b) change
  `placeSketchText` to emit/update the parametric sub-call; (c) hit-test +
  click-to-reopen mapping to the sub-call index; (d) an "update this text"
  commit path (per-index) alongside the existing "place new" path; (e) tests
  for create → reselect → edit → re-commit.

## Sequencing

1. Items 1–3 are pure UI relocations (no store/data-model change) and can ship
   together, ideally folded into one or two pinned overlays (a tool-param slot
   for the polygon, and a selection-transform panel for pattern + rotate).
2. Items 4 and 5 each require a parametric sub-call and draft-builder support;
   they are independent of each other and of items 1–3, and each should land as
   its own change with its own unit + E2E coverage because they alter the
   sketch data model.
