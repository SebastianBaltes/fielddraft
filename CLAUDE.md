# FieldDraft — Agent-Anweisungen

## Keine Backward Compatibility

Wir sind in einer frühen Dev-Phase. Schnelles Entwickeln und sauberer Code haben top Priorität vor Backward Compatibility. Keine Compat-Shims, keine Legacy-Fallbacks, keine alten Formate mitschleppen. Breaking Changes sind erlaubt und erwünscht, wenn sie den Code vereinfachen.

## Build & Test

- **Lokaler Lauf:** `just test` (Rust + Vitest). Playwright separat via `just test-e2e`.
- **Kernel only:** `cargo test --workspace`.
- **Web only:** `cd web && npm test`.
- **Dev-Server:** `just dev` (baut WASM und startet Vite auf :5173).
- **Voller Build:** `just build` (WASM + Vite Bundle).
- **Backend** (optional): `docker compose up --build -d db server` oder `cd server && go run .`.

Volle Deployment-Anleitung in `DEPLOY.md`.

## Release Notes pflegen

Bei jedem Versions-Bump (`web/package.json`) **einen Eintrag oben** in `web/src/releaseNotes.ts` ergänzen (neueste Version zuerst): `{ kind: "feat" | "fix" | "perf", text, scope?, tickets? }`. Diese Datei ist die handgepflegte SSOT für die In-App-Release-Notes (Menü „Release Notes…" + Klick aufs Versions-Badge). Die Historie wurde einmalig aus den Commits geseedet (`web/scripts/genReleaseNotes.mjs`) — das Skript nur zum kompletten Neu-Aufbau erneut laufen lassen, nicht im Build.

**Erzwungen** durch den `commit-msg`-Hook (`.githooks/commit-msg`, aktiv via `just setup-hooks` bzw. automatisch bei `just install`): ein Commit, dessen Subject `feat|fix|perf …(vX.Y.Z)` ist, wird abgelehnt, wenn `releaseNotes.ts` keinen Eintrag für diese Version hat. `docs/chore/refactor/test/revert/merge` und versionslose Commits sind ausgenommen (nicht release-note-würdig).

## Tickets abarbeiten (wenn nichts anderes ansteht)

User melden Fehler/Feature-Wünsche in-App über **Report Issue…** (F2). Tickets gehen IMMER an den **Server-Ticket-Store** (Go-Server, Backend IssueVault via `.env`); der frühere Vite-FS-Sink (`tickets/open/<id>/`, nur ohne `FIELDDRAFT_DEV_BACKEND=1`) wurde entfernt — auch `just dev` braucht fürs Ticket-Anlegen den laufenden Go-Server (`docker compose up -d db server`). Nur historische Tickets liegen noch als Ordner unter `tickets/done/`.

Wenn keine andere Aufgabe ansteht: offene Tickets abarbeiten — Ticket lesen, Screenshot/Attachments ansehen, Projekt zum Reproduzieren laden, Fix/Feature umsetzen, testen, committen+pushen.

**Beim Erledigen eines Tickets immer beides:**
1. **Ticket-Kommentar** hinterlassen, der den Fix/Status inhaltlich erklärt und **auf die GitHub-Commits verlinkt** (`https://github.com/SebastianBaltes/fielddraft/commit/<hash>`).
2. **Status setzen:** `resolved`.

Beides geht am einfachsten **in der App** (Issues-Tab: Kommentarfeld + „Set state") — der Go-Server schreibt dann garantiert das richtige Format. Wer stattdessen direkt an die IssueVault-REST-API geht (`POST /tickets/<id>/comments`, `PATCH /tickets/<id>`), muss den Vertrag selbst einhalten:
`author_kind: "agent"`, `meta: {author_type: "dev", internal: false, po_reviewed: true}` — **echte Booleans, keine 0/1**. IssueVault validiert `meta` nicht; eine 0 statt `false` ließ früher das Einlesen des ganzen Tickets scheitern (#345). Der Server liest solche Werte inzwischen nachsichtig, schreiben soll sie trotzdem niemand.

**Reihenfolge — nicht stur ältestes-zuerst, sondern nach Wirkung sortieren:**
1. **Regressionen zuerst** — was eine kürzliche Änderung kaputt gemacht hat, hat oberste Priorität (Arbeitsfähigkeit wiederherstellen).
2. **Blocker vor Kosmetik** — Crashes, Datenverlust, „Feature komplett kaputt" vor optischen/Komfort-Themen.
3. **Nach Subsystem gruppieren** — verwandte Tickets (z.B. mehrere Sketch-Tools) zusammen abarbeiten; eine Untersuchung deckt oft mehrere ab, und gemeinsame Ursachen werden sichtbar.
4. **Innerhalb einer Gruppe** Grundlegendes/Foundational vor Detail (z.B. „Linie persistiert nicht" vor „Linie snappt nicht").
5. Bei sonst gleichwertigen Tickets das ältere zuerst.

Mehrere Tickets desselben Schwungs dürfen in einem Rutsch (eigene Commits je Ticket) erledigt werden.

