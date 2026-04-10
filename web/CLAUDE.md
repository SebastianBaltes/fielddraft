## Styling

- Für Light Mode stylen 
- Generell jede Schriftgröße >= 1rem
- Styling lebt in CSS-Dateien, nicht in Inline-Styles und nicht in JS-Objekten. Inline-Styles sind erlaubt für rein dynamische Werte.
- CSS-Dateien sind hierarchisch organisiert unter `web/src/styles/`:
    - `tokens.css` — Farb-, Spacing-, Typografie-, Radius-, Shadow-Variablen. Single source of truth.
    - `base.css` — Browser-Resets, html/body, fokussierte und globale Defaults.
    - `layout/` — Shell und übrige Layout-Container.
    - `components/` — genau eine CSS-Datei pro Komponente, gleichnamig zur Komponente (`Toolbar.tsx` ↔ `toolbar.css`, `Outliner.tsx` ↔ `outliner.css`). 
- Eintrittspunkt ist `web/src/styles/index.css`, das alle Submodule per `@import` lädt. Komponenten importieren *nicht* einzeln, sondern konsumieren das aufgelöste Stylesheet via `main.tsx`.
- Native CSS Nesting statt BEM: Jede Komponente hat genau eine Top-Level-Klasse mit `fd-`-Präfix (`.fd-toolbar`, …). 
- Alle weiteren Selektoren der Komponente werden **innerhalb** dieser Top-Level-Klasse geschachtelt. Kind-Klassen bleiben kurz und unpräfixiert, weil sie durch das Nesting bereits gescoped sind (`.row`, `.header`, `.tool`, `.pick-hud`, …).
- CSS-Custom-Properties intensiv nutzen.
- Wenn du eine neue Komponente baust und merkst, dass dir ein Token fehlt: erst Token in `tokens.css` ergänzen, dann Komponente schreiben. Nicht andersrum.
