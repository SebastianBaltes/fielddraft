# FieldDraft — Pair-Programming-Notizen

Diese Datei ist Pflichtlektüre für jeden Coding-Agent (Claude o. ä.), der
am FieldDraft-Repo arbeitet. Die folgenden Regeln sind nicht verhandelbar.

## Styling

- **Light Mode**, immer. Dark Mode ist im MVP nicht vorgesehen. Farb-
  paletten werden aus dem Light-Mode-Token-Set in
  `web/src/styles/tokens.css` abgeleitet.
- **Keine Schrift kleiner als `1rem`**. Niemals. Auch nicht in Tooltips,
  nicht in Captions, nicht in Statusleisten. Fontgrößen werden über
  CSS-Variablen vergeben (`--fs-base`, `--fs-md`, `--fs-lg`, `--fs-xl`),
  und der untere Anschlag liegt bei `1rem` (= Browser-Default, typisch
  16 px). Wer kleinere Fonts braucht, hat das UX-Layout falsch designt
  und muss die Hierarchie überdenken statt den Font zu verkleinern.
- **Styling lebt in CSS-Dateien**, nicht in Inline-Styles und nicht in
  JS-Objekten. Inline-Styles sind erlaubt für rein dynamische Werte
  (z. B. `transform`-Animationen mit JS-berechneten Koordinaten), nicht
  für Layout, Farbe, Typografie, Spacing.
- CSS-Dateien sind **hierarchisch organisiert** unter `web/src/styles/`:
    - `tokens.css` — Farb-, Spacing-, Typografie-, Radius-, Shadow-
      Variablen. Single source of truth.
    - `base.css` — Browser-Resets, html/body, fokussierte und globale
      Defaults.
    - `layout/` — Shell, Topbar, Main-Split, Viewport-Container.
    - `components/` — Toolbar, Outliner, Buttons, Inputs etc.
  Eintrittspunkt ist `web/src/styles/index.css`, das alle Submodule per
  `@import` lädt. Komponenten importieren *nicht* einzeln, sondern
  konsumieren das aufgelöste Stylesheet via `main.tsx`.
- **CSS-Custom-Properties intensiv nutzen.** Jede wiederkehrende Größe
  (Farbe, Spacing, Schriftgröße, Border-Radius, Shadow, Transition)
  steht in `tokens.css` und wird per `var(--…)` referenziert.
  Hardcoded Hex-Werte oder px-Werte in Komponenten-CSS sind ein Bug.
- Wenn du eine neue Komponente baust und merkst, dass dir ein Token
  fehlt: erst Token in `tokens.css` ergänzen, dann Komponente schreiben.
  Nicht andersrum.

## Wenn du gegen eine dieser Regeln verstößt

Selbst-Refactor noch in derselben Iteration. Nicht "TODO später".
Geht nicht durch Review.
