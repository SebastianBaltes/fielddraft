# FieldDraft — Agent-Anweisungen

## Keine Backward Compatibility

Wir sind in einer frühen Dev-Phase. Schnelles Entwickeln und sauberer Code haben top Priorität vor Backward Compatibility. Keine Compat-Shims, keine Legacy-Fallbacks, keine alten Formate mitschleppen. Breaking Changes sind erlaubt und erwünscht, wenn sie den Code vereinfachen.

## Build & Test

- **Lokaler Lauf:** `just test` (Rust + Vitest). Playwright separat via `just test-e2e`.
- **Kernel only:** `cargo test --workspace`.
- **Web only:** `cd web && npm test`.
- **Dev-Server:** `just dev` (baut WASM und startet Vite auf :5173).
- **Voller Build:** `just build` (WASM + Vite Bundle).
- **Backend** (optional): `docker compose up --build -d` oder `cd server && go run .`.

Volle Deployment-Anleitung in `DEPLOY.md`.

## Tickets abarbeiten (wenn nichts anderes ansteht)

User melden Fehler/Feature-Wünsche in-App über **Report Issue…** (Menü oben links). Jedes Ticket landet unter `tickets/open/<id>/` (Screenshot, `project.fdraft`, `ticket.md`). Format und Ablauf: `tickets/README.md`.

Wenn keine andere Aufgabe ansteht: offene Tickets in `tickets/open/` abarbeiten (ältestes zuerst) — `ticket.md` lesen, Screenshot ansehen, `project.fdraft` zum Reproduzieren laden, Fix/Feature umsetzen, testen, committen+pushen, dann den Ordner nach `tickets/done/<id>/` verschieben und eine `resolution.md` (Commit-Hash + Kurzfassung) ergänzen.

