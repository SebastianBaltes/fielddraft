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

Bei jedem Versions-Bump (`web/package.json`) **einen Abschnitt oben** in `web/src/releaseNotes.md` ergänzen (neueste Version zuerst):

    ## 0.6.594
    - fix(scope) #ticket: Was der Nutzer davon merkt.

`kind` ist `feat | fix | perf`; `(scope)` und `#ticket` sind optional, der Text ist frei. Diese Datei ist **reiner Text und die SSOT** für die In-App-Release-Notes (Menü „Release Notes…" + Klick aufs Versions-Badge). `releaseNotes.ts` ist nur noch der **Parser** — dort steht kein Text mehr.

**Warum das wichtig ist:** Früher lagen die Sätze als TS-String-Literale in `releaseNotes.ts`. Damit war Prosa Code, und ein einziges ASCII-`"` in einem deutschen Satz beendete den String → `tsc` bricht ab → **der gesamte Prod-Build ist tot**. Das ist viermal passiert, zuletzt auf `main`. In der `.md` sind Anführungszeichen, Backslashes und Klammern **harmlos**. Eine kaputte Zeile kann die App auch nicht mehr leeren: der Parser sammelt sie in `malformed`, und `releaseNotes.test.ts` lässt CI daran scheitern — nicht den Nutzer.

Die Historie wurde einmalig aus den Commits geseedet (`web/scripts/genReleaseNotes.mjs > src/releaseNotes.md`) — das Skript nur zum kompletten Neu-Aufbau erneut laufen lassen, nicht im Build.

**Erzwungen** durch den `commit-msg`-Hook (`.githooks/commit-msg`, aktiv via `just setup-hooks` bzw. automatisch bei `just install`): ein Commit, dessen Subject `feat|fix|perf …(vX.Y.Z)` ist, wird abgelehnt, wenn `releaseNotes.md` keinen `## X.Y.Z`-Abschnitt hat. `docs/chore/refactor/test/revert/merge` und versionslose Commits sind ausgenommen (nicht release-note-würdig).

## Tickets abarbeiten (wenn nichts anderes ansteht)

User melden Fehler/Feature-Wünsche in-App über **Report Issue…** (F2). Tickets liegen AUSSCHLIESSLICH in **IssueVault** — der Go-Server hat seit #346 kein anderes Ticket-Backend mehr (kein SQL-, kein FS-Fallback, kein `TICKET_BACKEND`-Schalter, keine `tickets/`-Ordner im Repo). Auch `just dev` braucht fürs Ticket-Anlegen den laufenden Go-Server (`docker compose up -d db server`) plus `ISSUEVAULT_TOKEN` in der `.env`.

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

