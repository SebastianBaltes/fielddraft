# FieldDraft — Agent-Anweisungen

## Telegram-Kommunikation

Du hast zwei MCP-Tools für die Kommunikation:
- **`send_telegram`** — Statusmeldungen (fire-and-forget, Arbeit läuft weiter)
- **`request_feedback`** — Frage an den User, **Script wartet auf Antwort** bevor es weitergeht

Nutze sie aktiv:

### Wann melden:
1. **Sofort bei Arbeitsbeginn:** Was du als nächstes angehst. Z.B. `🔨 Starte K8.13: Sketch-Region-Selektion für Multi-Profile Extrude`
2. **Bei längeren Tasks alle ~5 Minuten:** Kurzer Zwischenstand. Z.B. `⏳ K8.13: Region-Zerlegung im Kernel implementiert, jetzt UI-Picker...`
3. **Nach jedem Meilenstein:** Zusammenfassung. Was gebaut, Tests grün?, was kommt als nächstes.
4. **Bei Problemen:** Was schiefgeht, was du versucht hast, ob du Hilfe brauchst.
5. **Bei Design-Entscheidungen:** Wenn du von der README abweichst oder eine Alternative wählst.

### Format:
- Kurz und knapp (2-3 Sätze max)
- Emoji am Anfang für schnelles Scannen:
  - 🔨 = starte neuen Task
  - ⏳ = Zwischenstand bei laufender Arbeit
  - ✅ = Meilenstein fertig
  - ⚠️ = Problem, brauche evtl. Hilfe
  - ❓ = Frage / Design-Entscheidung
  - 🔧 = Refactoring / Aufräumen

Für Fragen die eine Antwort brauchen, nutze `request_feedback`.

### Nicht melden:
- Jeden einzelnen Datei-Edit
- Triviale Einzeiler-Fixes
- Compiler-Warnungen die du sofort behebst

## Dokumentationsstruktur (wichtig für Token-Haushalt)

Die Projektdokumentation ist seit April 2026 in **fünf Dateien aufgeteilt**. Lade nur das, was du tatsächlich brauchst — vermeide es, mehrere dieser Dateien in derselben Session zu laden, wenn eine reicht.

| Datei | Inhalt | Wann lesen | Cache-Profil |
|---|---|---|---|
| `README.md` | Vision, Zielgruppe, Funktionsumfang, Quick Start, §5–§9, §12–§15 | Onboarding, Status-/Scope-Fragen | **stabil** (ändert sich selten) |
| `docs/ARCHITECTURE.md` | §4.x Technische Architektur + §11 Risiken | Kernel-/Frontend-Arbeit, Algorithmen-Design | **stabil** (ändert sich mit Architektur-Entscheidungen) |
| `docs/ROADMAP.md` | K-Slices, Tiers, M-Meilensteine, Phase 3 | Slice-Planung, Slice-Reihenfolge | **semi-stabil** (ändert sich bei neuen Slices wie K3.5) |
| `docs/LITERATURE.md` | §16 Pflichtlektüre pro Themengebiet | Nur beim Implementieren eines spezifischen Algorithmus — nur den relevanten Abschnitt | **stabil** |
| `docs/STATUS.md` | Ist-Zustand (done/partial/open), Test-Zahlen, Empfehlung | Status-Fragen, Slice-Fortschritt | **volatil** (ändert sich pro Meilenstein) |

### Token-Caching-Strategie

Anthropic cached Prompt-Inhalte 5 Minuten lang. Die Dateistruktur ist so gewählt, dass **stabile Dateien zusammen cacheable** sind und **volatile Dateien separat** liegen:

- **Beim Onboarding:** Lade **nur `README.md`** (~10k Tokens). Das reicht für Vision, Scope und Quick Start.
- **Für Architektur-Arbeit:** Lade zusätzlich **`docs/ARCHITECTURE.md`** (~25k Tokens). Diese Datei ist stabil und cached gut über Sessions hinweg.
- **Für Slice-Planung:** Lade **`docs/ROADMAP.md`** (~15k Tokens). Nicht zusammen mit LITERATURE, außer beide werden gebraucht.
- **Für Algorithmen-Implementierung:** Lade **nur den relevanten §16.x-Abschnitt aus `docs/LITERATURE.md`** (per `offset`/`limit` im Read-Tool). Nicht die ganze Datei.
- **Für Status-Fragen:** Lade **nur `docs/STATUS.md`**. Diese Datei ist volatil — sie wird oft aktualisiert, Cache-Hits sind unwahrscheinlich, aber die Datei ist klein.

**Anti-Pattern:** Lade nicht alle Doc-Dateien auf Verdacht. Die frühere monolithische `README.md` mit ~75k Tokens wurde bewusst aufgeteilt, um genau dieses Gesamtladen zu vermeiden. Wenn du dich dabei erwischst, `README.md` + `docs/ARCHITECTURE.md` + `docs/ROADMAP.md` + `docs/LITERATURE.md` gleichzeitig zu laden, überlege, ob du wirklich alle brauchst — meistens reicht eine oder zwei davon für die aktuelle Aufgabe.

## Roadmap und Slice-Arbeit

Die Roadmap steht in **`docs/ROADMAP.md`**. Arbeite die Meilensteine in der dort definierten Reihenfolge ab. Der aktuelle Ist-Stand steht in **`docs/STATUS.md`** — dort siehst du, welche Slices `done`/`partial`/`open` sind.

**Am Ende von `docs/STATUS.md` steht meistens eine Empfehlung zur weiteren Entwicklung**, die die Reihenfolge der als nächstes abzuarbeitenden Slices aus der Roadmap begründet und priorisiert. Lies diesen Abschnitt, bevor du einen neuen Slice startest — die Reihenfolge dort hat Vorrang vor der rohen ROADMAP-Reihenfolge, weil sie aktuelle Blocker und strategische Umpriorisierungen berücksichtigt.

**Nach jedem erledigten Slice/Meilenstein musst du `docs/STATUS.md` aktualisieren:**
1. Den Slice-Eintrag in der Tier-Tabelle auf ✅ setzen und die Kurzbeschreibung auf das tatsächlich ausgelieferte Ergebnis anpassen (inkl. Commit-Hash, falls hilfreich).
2. Die "Empfehlung zur weiteren Entwicklung" am Ende aktualisieren: erledigte Sub-Slices markieren, den nächsten fälligen Slice als **nächster Slice** kennzeichnen, falls nötig die Reihenfolge neu begründen.
3. Tier-Fortschrittszahlen im "Zusammenfassung nach Tiers"-Block prüfen und anpassen, wenn sich die Done-Zahl verschoben hat.

Danach: `just test` (oder `cargo test` für Rust-only), committen, Telegram-Meldung.

Für die **architektonische Begründung** eines Slices (warum ist der CSG-Graph so aufgebaut, was macht die Ghost Topology, warum Newton-Projektion in K3.5) → **`docs/ARCHITECTURE.md`**. Für die **Literatur** (Dual-Contouring-Paper, QEF-Solver, PlaneGCS, ROAM für K3.5/K12) → **`docs/LITERATURE.md`**.

## Build & Test

- **Lokaler Lauf:** `just test` (Rust + Vitest). Playwright separat via `just test-e2e`.
- **Kernel only:** `cargo test --workspace`.
- **Web only:** `cd web && npm test`.
- **Dev-Server:** `just dev` (baut WASM und startet Vite auf :5173).
- **Voller Build:** `just build` (WASM + Vite Bundle).
- **Backend** (optional): `docker compose up --build -d` oder `cd server && go run .`.

Volle Deployment-Anleitung in `DEPLOY.md`.

## Chatverlauf

ClaudeDraft speichert jede Nachricht zwischen dir und dem Nutzer persistent in
einer SQLite-DB. Dieser Verlauf ueberlebt `/clear`, Neustarts des Daemons und
einen Umzug zwischen Nodes — der frische Claude-Prozess kennt ihn aber zunaechst
nicht. Wenn der Nutzer sich auf etwas Aelteres bezieht oder du nach einem
`/clear` den Kontext rekonstruieren willst, ruf das MCP-Tool `chat_history`
auf (liefert die letzten N Nachrichten als {role, text, ts}).
