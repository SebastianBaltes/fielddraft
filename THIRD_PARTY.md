# Drittkomponenten / Third-party components

FieldDraft selbst steht unter MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)). Die folgenden Komponenten behalten ihre
eigenen Lizenzen. Stand 2026-10-21, direkte Abhängigkeiten; transitive
Abhängigkeiten siehe `Cargo.lock`-Auflösung bzw. `web/package-lock.json`.

## Nicht-permissive Komponenten

| Komponente | Lizenz | Einbindung |
|---|---|---|
| [CGAL](https://www.cgal.org/) (Mesh_3) | GPL-3.0-or-later | nur im separaten Server-Mesher `cgal-mesher/`, eigener Prozess/Container; dieses Verzeichnis steht deshalb unter GPL-3.0-or-later ([cgal-mesher/LICENSE](cgal-mesher/LICENSE)) |
| [PlaneGCS](https://github.com/Salusoft89/planegcs) (`@salusoft89/planegcs`, aus FreeCAD) | LGPL-2.0-or-later | npm-Modul, als eigenständiges WASM-Modul geladen, nicht verändert |
| [opencascade.js](https://github.com/donalffons/opencascade.js) | LGPL-2.1-only | npm-Modul, als eigenständiges WASM-Modul im Web Worker geladen |
| [occt-import-js](https://github.com/kovacsv/occt-import-js) | LGPL-2.1 | npm-Modul, als eigenständiges WASM-Modul geladen |
| [fidget](https://github.com/mkeeter/fidget) | MPL-2.0 | Rust-Crate, unverändert |

## Rust-Crates (direkt)

| Crate | Version | Lizenz |
|---|---|---|
| `baby_shark` | 0.3.12 | MIT |
| `cavalier_contours` | 0.7.0 | MIT OR Apache-2.0 |
| `console_error_panic_hook` | 0.1.7 | Apache-2.0/MIT |
| `fast-surface-nets` | 0.2.1 | MIT OR Apache-2.0 |
| `fidget` | 0.3.9 | MPL-2.0 |
| `i_overlay` | 7.0.2 | MIT OR Apache-2.0 |
| `js-sys` | 0.3.94 | MIT OR Apache-2.0 |
| `kiddo` | 5.3.2 | MIT OR Apache-2.0 |
| `nalgebra` | 0.34.2 | Apache-2.0 |
| `ndshape` | 0.3.0 | MIT OR Apache-2.0 |
| `num-dual` | 0.11.2 | MIT OR Apache-2.0 |
| `proc-macro2` | 1.0.106 | MIT OR Apache-2.0 |
| `quote` | 1.0.45 | MIT OR Apache-2.0 |
| `rayon` | 1.12.0 | MIT OR Apache-2.0 |
| `rmp-serde` | 1.3.1 | MIT |
| `serde` | 1.0.228 | MIT OR Apache-2.0 |
| `serde-wasm-bindgen` | 0.6.5 | MIT |
| `serde_json` | 1.0.149 | MIT OR Apache-2.0 |
| `smol_str` | 0.3.6 | MIT OR Apache-2.0 |
| `spade` | 2.15.1 | MIT OR Apache-2.0 |
| `syn` | 2.0.117 | MIT OR Apache-2.0 |
| `wasm-bindgen` | 0.2.117 | MIT OR Apache-2.0 |

## npm-Pakete (`web/`, direkt, Laufzeit)

| Paket | Version | Lizenz |
|---|---|---|
| `@ai-sdk/anthropic` | 3.0.96 | Apache-2.0 |
| `@ai-sdk/google` | 3.0.91 | Apache-2.0 |
| `@ai-sdk/openai` | 3.0.84 | Apache-2.0 |
| `@openrouter/ai-sdk-provider` | 2.10.0 | Apache-2.0 |
| `@react-three/drei` | 10.7.7 | MIT |
| `@react-three/fiber` | 9.5.0 | MIT |
| `@salusoft89/planegcs` | 1.1.7 | LGPL-2.0-or-later |
| `acorn` | 8.17.0 | MIT |
| `acorn-typescript` | 1.4.13 | MIT |
| `ai` | 6.0.224 | Apache-2.0 |
| `dompurify` | 3.4.12 | (MPL-2.0 OR Apache-2.0) |
| `fflate` | 0.8.3 | MIT |
| `html-to-image` | 1.11.13 | MIT |
| `marked` | 17.0.6 | MIT |
| `mermaid` | 11.16.0 | MIT |
| `monaco-editor` | 0.55.1 | MIT |
| `occt-import-js` | 0.0.23 | LGPL-2.1 |
| `opencascade.js` | 2.0.0-beta.b5ff984 | LGPL-2.1-only |
| `opentype.js` | 2.0.0 | MIT |
| `react` | 19.2.5 | MIT |
| `react-dom` | 19.2.5 | MIT |
| `sucrase` | 3.35.1 | MIT |
| `three` | 0.184.0 | MIT |

## Go-Module (`server/`, direkt)

| Modul | Lizenz |
|---|---|
| `github.com/go-sql-driver/mysql` | MPL-2.0 |
| `golang.org/x/crypto` | BSD-3-Clause |

## Assets

| Datei | Herkunft | Lizenz |
|---|---|---|
| `web/public/fonts/DejaVu*.ttf` | [DejaVu Fonts](https://dejavu-fonts.github.io/) | Bitstream Vera / DejaVu-Lizenz, siehe `web/public/fonts/LICENSE.txt` |
| `assets/models/stanford-bunny.stl` | [Stanford 3D Scanning Repository](https://graphics.stanford.edu/data/3Dscanrep/) | frei für Forschung und Lehre; Quellenangabe erbeten |
| `assets/models/utah-teapot-nonmanifold.stl` | Utah Teapot (Martin Newell, 1975) | gemeinfrei/frei verwendbar |
| `third_party/baseaichat` | Git-Submodul [BaseAIChat](https://github.com/SebastianBaltes/BaseAIChat) | siehe dort |
