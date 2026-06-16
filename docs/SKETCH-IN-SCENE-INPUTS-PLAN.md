# Sketch In-Scene Inputs — Integration Plan

> **Status: ALL ITEMS IMPLEMENTED** (v0.6.158–0.6.160). Items 1–3 (n-gon /
> pattern / rotate) are in-scene overlays (v0.6.158). Items 4 (parametric
> fillet/chamfer re-edit, v0.6.159) and 5 (parametric text re-edit, v0.6.160)
> were implemented as a **web-only tagged-recipe hybrid** — the baked geometry
> carries an additive, kernel-ignored recipe that round-trips verbatim — rather
> than the parametric-kernel approach this doc originally proposed. Sections 4
> and 5 below document the implemented hybrid; the chained-fillet and
> font-must-be-loaded limitations are noted there.

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

## 4. Post-hoc per-entity fillet/chamfer radius (tagged-recipe hybrid) — IMPLEMENTED

The ticket's literal ask: "click the fillet rounding to edit its radius
in-scene," the way a line's length is click-editable. The original plan here
proposed making corners *parametric* (a `sketch_fillet` / `sketch_chamfer`
sub-call re-derived in `buildDraftSketchState`), which would have touched the
sketch data model on both the web and kernel sides. **We shipped a lower-risk
WEB-ONLY hybrid instead — the kernel is untouched.**

### Why the hybrid is safe

`FeatureCall.params` in the kernel is a schema-less `serde_json::Value`, and
the kernel's `SketchArcDef::parse` / `SketchLineDef::parse` ignore unknown
fields (no `deny_unknown_fields`). Sub-calls round-trip VERBATIM through
`replaceSketch` → store → `getSketchSubCalls`, so an extra param attached to a
baked sub-call survives commit/reopen. The kernel ignores it for geometry; only
the web reads it to re-edit. This makes the recipe purely additive on the
params Value — geometry and entity-id allocation are unchanged.

### What was built

The baked flow is unchanged in shape: `store.filletSketchAt` still trims the
two source lines in place and appends a plain `sketch_arc`; `chamferSketchAt`
appends a `sketch_line` bevel. The addition is a **re-edit recipe tag** on the
baked geometry:

- **Stable ids.** Sketch sub-calls carry no `id` field (a `SubCall` is just
  `{ type, params }`, and entity allocation depends on spine *order*). Adding a
  real id across the kernel round-trip would have been the big change we were
  avoiding, so instead each relevant sub-call gets an additive `fd_id` string
  param (`FD_ID_PARAM` in `sketchFillet.ts`). The two trimmed source lines and
  the appended arc/bevel are all tagged in one `filletSketchAt`/`chamferSketchAt`
  call; references are by `fd_id`, not array index, so they survive reordering.
- **Recipe tag.** The appended arc carries `fillet: { line_a_id, line_b_id,
  radius, orig_a, orig_b }`; the appended bevel carries `chamfer: {
  line_a_id, line_b_id, distance, orig_a, orig_b }`. `orig_a` / `orig_b` are the
  ORIGINAL (pre-trim) line geometries `{ a:[x,y], b:[x,y] }`, captured before
  trimming, so a radius/distance change re-derives from scratch. Builders and
  readers: `filletRecipe` / `filletRecipeOf` (`sketchFillet.ts`) and
  `chamferRecipe` / `chamferRecipeOf` (`sketchChamfer.ts`).
- **Hit-test → corner.** A fillet arc is NOT a picked solver entity (the draft
  builder only allocates its centre point), so `hitTestSketchEntity` would never
  reach it. Instead `hitTestTaggedCorner(subCalls, cursor, tolMm)` in
  `sketchHitTest.ts` reads the sub-call geometry directly (`curveOfSubCall` +
  `distToCurve`), finds the nearest recipe-tagged arc/bevel within tolerance,
  and returns its `fd_id`, current value, and anchor. The Select-mode click path
  in `SketchCanvas2d.tsx` calls it before the entity hit-test.
- **Anchored editor.** Reuses the line-length editor pattern + its
  `.fd-sketch-live-input` styling. Anchor: the fillet arc's midpoint `(u, v)`
  for a fillet, the bevel line's midpoint for a chamfer (projected to client px
  via `sketchToScreen`). Enter commits, Esc cancels, and it is dismissed on
  zoom / tool-change like the existing editors. Testids:
  `sketch-fillet-edit-radius`, `sketch-chamfer-edit-distance`.
- **Per-entity setters.** `store.setFilletRadiusAt(arcSubCallId, newRadius)`
  and `store.setChamferDistanceAt(bevelSubCallId, newDistance)` find the tagged
  sub-call by `fd_id`, re-run `filletLines(orig_a, orig_b, …)` /
  `chamferLines(…)` (pure reducers `reeditFilletRadius` / `reeditChamferDistance`),
  rewrite the two referenced line sub-calls and the arc/bevel (with a refreshed
  recipe) in place, then go through `replaceSketchSubCalls` + the normal
  `solveAndReplace`. `rebuildSubCallsFromSolved` spreads `...call.params`, so the
  recipe + `fd_id` survive the re-solve.

### Known limitation — chained fillets

`orig_a` / `orig_b` snapshot the source lines as they were at FIRST creation.
If a later fillet/chamfer trimmed one of those same lines again, re-editing this
corner restores that line to its first-creation length (it does not see the
second trim). Re-edit chained corners outermost-first, or recreate the corner.
This is noted in code comments on `reeditFilletRadius` / `reeditChamferDistance`
and is intentionally not over-engineered.

### Tests

Pure-reducer unit tests live next to the existing fillet/chamfer tests
(`sketchFillet.test.ts`, `sketchChamfer.test.ts`): bake a recipe-tagged corner,
read the recipe back, call `reeditFilletRadius` / `reeditChamferDistance` with a
new value, and assert the arc/bevel + both trimmed lines update (and that the
re-derivation uses the ORIGINAL geometry, plus the null-return edge cases). The
store setters are thin wrappers over these reducers plus a re-solve.

## 5. Re-edit committed TEXT in-scene (tagged-recipe hybrid) — IMPLEMENTED

A committed text is exploded to per-glyph `sketch_path` contours
(`store.placeSketchText` → one `sketch_path` per glyph, "carrying its outer
ring + holes"). The original string, height, and font are **not recoverable**
from the contours. The original plan here proposed a parametric `sketch_text`
sub-call whose glyphs are *derived* in `buildDraftSketchState` — that would have
touched the sketch data model AND required font support on the kernel side.
**We shipped the same lower-risk WEB-ONLY hybrid as item 4 — the kernel is
untouched.**

### Why the hybrid is safe

Identical rationale to item 4: `FeatureCall.params` is a schema-less
`serde_json::Value`, and the kernel's `sketch_path` parse ignores unknown
fields (no `deny_unknown_fields`). Sub-calls round-trip VERBATIM through
`replaceSketch` → store → `getSketchSubCalls`, so an extra param attached to a
baked glyph survives commit/reopen. The kernel ignores it for geometry; only
the web reads it to re-edit. No Rust font support is needed — re-derivation
runs entirely in the browser from the loaded `opentype.Font`.

### What was built

- **Recipe tag.** Each glyph `sketch_path` of one placement carries an additive
  `text: { group, value, height, fontId, origin:[u,v] }` param, where `group`
  is a single fresh id (`nextFdId()`) shared by ALL glyphs of that text. The
  glyphs also get a per-sub-call `fd_id`, but `text.group` is what ties them
  together. Builder/reader: `textToSubCalls(font, value, { …, fontId })` now
  stamps the recipe, and `textRecipeOf(call)` reads it back (`sketchText.ts`).
  `nextFdId` / `FD_ID_PARAM` are reused from `sketchFillet.ts` (no duplication).
  Geometry/contours are unchanged — purely additive on the params Value.
- **Hit-test → text group.** A baked glyph is NOT a picked solver entity (the
  draft builder tessellates `sketch_path` into a region, not pick targets), so
  `hitTestSketchEntity` never reaches it — mirroring item 4. `hitTestTaggedText`
  (`sketchHitTest.ts`) instead reads the glyph's bounding box directly
  (`pathBBox` in `sketchText.ts`) grown by the pick tolerance: a click inside
  (or just outside) any letter returns that glyph's text group + the stored
  `value`/`height`/`fontId`/`origin`. **Select-mode click priority:** points/
  entities are handled inside `hitTestSketchEntity`; the click path runs
  `hitTestTaggedCorner` (small precise targets) FIRST, then `hitTestTaggedText`
  (clicking glyph ink should edit the text, not pick the region behind it), then
  the entity hit-test.
- **Reopen the editor pre-filled.** Clicking tagged text opens the EXISTING
  `.fd-sketch-text-editor` overlay at the recipe `origin` (projected to client
  px via the same `sketchToScreen` the corner editor and the create-text editor
  use), pre-filled with the stored value, and the params popover showing the
  stored height/font. The editor's `textEditor` state gained a `group` field:
  `null` = the existing "place new" mode, a set group = "edit existing" mode.
  Opening pushes the stored value/height/font into the store params (the editor
  fields read from there, like the create flow).
- **Commit path: update the group in place.** `store.updateSketchTextGroup(
  groupId, { value, height, fontId })` finds every glyph whose `text.group ===
  groupId`, re-runs `reeditTextGroup` (`sketchText.ts`) — which re-lays the
  glyphs via `textToSubCalls` from the recipe's `origin`, re-stamps the SAME
  group id, and splices the fresh glyphs in at the first old glyph's position
  (surrounding sub-calls preserved by reference) — then goes through
  `replaceSketchSubCalls` + the normal `solveAndReplace`. The editor's
  `commitTextEditor` dispatches to `updateSketchTextGroup` in edit mode, else
  `placeSketchText` (the create path is unchanged).

### Font-availability handling

Re-deriving needs the parsed font in the cache. `updateSketchTextGroup` reads
`cachedFont(fontId)`; if the font isn't ready it kicks `ensureTextFontLoaded`
and returns false (a graceful no-op) — **the old geometry is kept** and the
user can retry once the font settles (the existing "Loading font…" hint in the
params popover surfaces the state). The recipe stores the original `fontId`, so
re-editing keeps the original font as long as it's still resolvable; if the user
switches to a different font in the popover, that becomes the new recipe font.
A genuinely-gone font id (e.g. an uploaded font lost across a reload) simply
fails to load → the edit no-ops rather than corrupting the text; switching to a
bundled font in the popover recovers it.

### Tests

Pure-reducer unit tests live in `sketchText.test.ts`: place tagged text, read
the recipe off the glyphs, assert all glyphs share one group, then
`reeditTextGroup` with a new value/height and assert the glyphs changed, all
carry the SAME group id + new value, re-derive from the recipe origin, splice in
place, and the null edge cases (unknown group, geometry-less value). `pathBBox`
is covered too. `hitTestTaggedText` is unit-tested in `sketchHitTest.test.ts`
(font-free hand-built tagged glyph): inside-bbox hit, outside-tolerance miss,
within-tolerance edge hit, and ignoring untagged paths. An E2E for the edit flow
is optional and was not added (the create-flow e2e in `tests/sketch-text.spec.ts`
is unchanged).

### Known limitation

Same web-only spirit as item 4: the recipe is the source of truth for re-editing,
so a glyph edited by hand (e.g. node-dragging an individual contour) is NOT
reconciled back into the recipe — re-editing the group re-lays all glyphs from
the stored value/height/font/origin and discards such manual tweaks. Re-layout
also uses the recipe `origin`, so the text does not move on a value/height/font
change (move is a separate, not-yet-built gesture).

## Sequencing

1. Items 1–3 are pure UI relocations (no store/data-model change) and can ship
   together, ideally folded into one or two pinned overlays (a tool-param slot
   for the polygon, and a selection-transform panel for pattern + rotate).
2. Items 4 and 5 both shipped as the web-only tagged-recipe hybrid above (kernel
   untouched), each with pure-reducer + hit-test unit coverage. Item 5 followed
   item 4's vocabulary exactly: a `text` recipe param (shared `group` id) on the
   baked glyph contours, re-derived in the browser via `reeditTextGroup`, rather
   than the parametric `sketch_text` sub-call originally sketched there.
