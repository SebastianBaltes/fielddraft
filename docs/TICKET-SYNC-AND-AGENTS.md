# FieldDraft — Ticket-Sync & die zwei Prod-Agenten (Architektur & Intention)

> **Status (Ticket #346): der FS-Teil dieses Entwurfs ist Geschichte.** Tickets
> liegen nur noch in IssueVault; der Go-Server hat kein SQL- und kein
> Datei-Backend mehr, und die materialisierten `tickets/open|done/<id>/`-Ordner
> sind aus dem Repo entfernt. Agenten lesen und schreiben Tickets über die App
> bzw. die IssueVault-REST-API (Vertrag: siehe CLAUDE.md). Was unten über die
> Datei-Materialisierung und den Snapshot-Sync steht, beschreibt den früheren
> Stand und ist nur noch historisch zu lesen.


> Wie die Desktop-Agenten (`fielddraft-po`, `fielddraft-coding`) an die Tickets
> des **Prod-Systems** kommen, ohne die Ergonomie des direkten Datei-Zugriffs zu
> verlieren — und wie die zwei Agenten dabei getrennt und der PO source-los
> bleiben. Ergänzt `docs/PLATFORM-ARCHITECTURE.md` (dort: das Plattform-Gesamtbild,
> §6/§8/§9 Agent-API + PO-Gateway).
>
> Status: **Architektur-/Intentions-Dokument** für den in §7 abgegrenzten Bau.

---

## 1. Intention (das Warum)

FieldDraft erprobt ein **User→Agent-Entwicklungsmodell**: eingeladene Freunde
melden auf der Live-App Tickets, ein **Product-Owner-Agent (PO, Opus 4.8)**
triagiert und kommuniziert im Thread, ein **Coding-Agent** baut/testet/deployt.
Kein menschliches Gate.

Zwei harte Randbedingungen prägen diesen Baustein:

1. **Die Ergonomie des Agenten ist direkter Datei-Zugriff.** Heute (lokaler
   Datei-Flow) kann der Agent `rg` über alle Tickets laufen lassen, `ticket.md`
   catten, den Screenshot ansehen, `project.fdraft` laden, `actions.json`
   inspizieren — reibungslos. **Das muss erhalten bleiben**, auch wenn die
   Tickets in Prod in einer DB liegen. → Der Agent arbeitet weiter **nur über
   Dateien**; die DB wird dorthin *materialisiert*.
2. **Der PO bleibt strukturell source-los** („kein Source raus", §4 der
   Plattform-Architektur). Er darf das Repo nicht sehen — auch nicht dadurch,
   dass er zufällig in der Repo-Worktree läuft.

Die **DB ist die geteilte, transaktionale, nebenläufige Wahrheit** (viele Freunde
schreiben gleichzeitig; State-Maschine + append-only-Thread brauchen Integrität).
**Dateien sind die Arbeitsfläche des Agenten** — eine lokale, materialisierte
Sicht, kein zweiter Wahrheits-Store.

---

## 2. Warum kein rsync, warum kein bidirektionales File-Sync

**Prod-Tickets sind keine Dateien.** Auf dem V-Server liegen sie als
**DB-Zeilen** (`tickets`, `ticket_comments`, State, Verdikt) **+ content-addressed
Blobs** (`/data/blobs/<sha256>`, dedupliziert). Es gibt dort **kein**
`tickets/<id>/`-Verzeichnis.

- **rsync kann keine MariaDB-Zeilen lesen.** Titel/Body/Thread/State entstehen
  erst durch Lesen der DB. Die Blobs liegen flach nach `sha256` — welcher Blob zu
  welchem Ticket / `kind` / Dateinamen gehört, weiß **nur die DB**. Ein rsync
  liefert einen sha256-Haufen, keinen Ticket-Ordner.
- **Prod file-basiert zu machen, nur damit rsync geht**, wäre ein schlechter
  Tausch: der Multi-User-Server braucht die DB (nebenläufige Schreibzugriffe,
  Transaktionen, „nur meine"-Queries, State-Maschine).
- **Bidirektionales rsync ist konfliktträchtig.** Ändert ein User auf Prod einen
  Thread, während der Agent lokal schreibt, überschreibt rsync eine Seite
  (**lost update**). rsync ist kein Merge-Tool; für nebenläufige Multi-Writer-Daten
  ist es das falsche Werkzeug.

---

## 3. Der Sync-Layer: „Snapshots runter, Kommandos hoch"

Statt symmetrischem File-Sync ein **asymmetrisches** Modell, das genau zur
Datennatur passt:

```
              PROD (V-Server)                          DESKTOP (Agent)
  ┌───────────────────────────────┐       pull        ┌───────────────────────┐
  │ DB: ticket/comments/state      │  ── snapshot ──▶  │ tickets/open/<id>/     │
  │ Blobs: /data/blobs/<sha256>    │  (immutable art.) │   (Dateien: rg/cat/…)  │
  │                                │                   │                        │
  │  ◀── semantische Operationen ──┼───── push ────────┤ claim/verdict/comment/ │
  │      (transaktional, API)      │                   │ state/complete/deploy  │
  └───────────────────────────────┘                   └───────────────────────┘
```

**Runter (pull, read-only, idempotent):**
- **Artefakte** (Screenshot, `project.fdraft`, `actions.json`, Settings, UI-State,
  Attachments) sind **content-addressed und immutable** → einmal ziehen,
  kollidieren nie; Re-Pull ist ein reines Refresh.
- **Ticket-Text + Thread** kommen als **frischer Snapshot** (überschreibt die
  lokale Lese-Sicht; die lokale Sicht ist nie die Wahrheit).

**Hoch (push): keine Dateien, sondern getippte API-Operationen** —
`claim` / `verdict` / `agent-comment` / `patch-state` / `complete` / `deploy`.
Der Server wendet sie **transaktional** an (State-Maschine-geschützt, Kommentare
append-only). Kein File-Merge → **kein Lost-Update**, selbst wenn parallel ein
User im Thread schreibt.

**Warum das sauber ist:** Immutable Daten fließen nur runter (kollisionsfrei),
mutable Daten fließen als *Operationen* hoch (der Server ist die einzige Instanz,
die den State fortschreibt). Kein Unison/CRDT-Gefrickel nötig.

### 3.1 Materialisiertes Layout (was der Agent sieht)

```
tickets/open/<id>/
  ticket.md        # Titel + Body + Meta (ID/Version/Zeit/URL/UA) — wie heute
  comments.md      # der Thread: user / po / dev / system, chronologisch
  ticket.json      # state, category, po_* Verdikt, author, ids — für Push-Kontext
  screenshot.png   # ← artifacts/{aid} (kind=screenshot)
  project.fdraft   # ← kind=fdraft
  actions.json     # ← kind=actions
  settings.json    # ← kind=settings
  ui_state.json    # ← kind=ui_state
  attachment-NN-*  # ← kind=attachment (Original-Dateiname)
tickets/done/<id>/ # nach Abschluss verschoben (+ resolution im Thread/ticket.json)
```

Das ist **byte-nah** das Layout, das der file-basierte Flow heute erzeugt — der
Agent merkt keinen Unterschied zwischen „lokales Datei-Ticket" und
„materialisiertes Prod-Ticket". `rg "…" tickets/open/` greift wie gewohnt über
`ticket.md`/`comments.md`/`actions.json`; Screenshots werden angesehen (binär,
nicht gegrept — wie heute); `project.fdraft` wird zum Reproduzieren geladen.

### 3.2 Das Werkzeug

Ein dünnes CLI/Modul (Node-ESM, zero-dep, im `agents/`-Stil, baut auf
`agents/lib/apiClient.mjs`):

- `pull [--scope po|coding]` → `GET /api/agent/queue`, lädt je Ticket die
  Artefakte via `GET /api/tickets/{id}/artifacts/{aid}` und schreibt das Layout.
- Push-Ops als dünne Wrapper der API: `claim`, `verdict`, `comment`, `state`,
  `complete`, `deploy` (verwenden die schon existierenden Endpoints, WP6/WP8).
- Konfiguration über Env: `FIELDDRAFT_API_URL`, `PO_SERVICE_TOKEN` /
  `CODING_SERVICE_TOKEN`, Ziel-Verzeichnis.

Die API dafür existiert bereits (queue mit Thread+Artefakt-Refs,
Artefakt-Download, verdict/comment/state/complete/deploy) — es ist **Plumbing,
keine neue Infrastruktur.**

---

## 4. Die zwei Prod-Agenten (getrennte Arbeitsverzeichnisse)

Beide laufen als **ClaudeDraft-Agenten auf dem Desktop**, aber in **getrennten
Working-Dirs** — so bleibt der PO source-los, obwohl beide auf derselben Maschine
sind.

### 4.1 `fielddraft-po` (Opus 4.8, source-los)
- **Working-Dir:** ein **reines Tickets-Verzeichnis** (z. B. `~/fd-po-tickets/`) —
  **keine** Repo-Worktree, kein Source.
- **Token:** `PO_SERVICE_TOKEN` (Ticket-Scope, kein Repo/Deploy).
- **Zyklus:** `pull --scope po` (materialisiert **Ticket-Text + Thread +
  Screenshot** — genug zum Triagieren/Antworten/Spam-Beurteilen) → klassifiziert
  `spam` / `malicious` / `vision_fit` → `verdict` (setzt State, schreibt
  `public_reply` durch den Leak-Filter, legt bei `accepted` die Work-Order an) →
  reicht die **Artefakt-Refs** in der Work-Order an den Coding-Agenten weiter
  (Gateway: direkt durchreichen, nicht paraphrasieren, §9 Plattform-Arch).
- **Kein** `project.fdraft`/Repo nötig — der PO ist ein Text-/Triage-Job.

### 4.2 `fielddraft-coding` (in der Repo-Worktree)
- **Working-Dir:** die **Repo-Worktree** (mit Source, Build, Deploy-Keys).
- **Token:** `CODING_SERVICE_TOKEN` (+ Repo/GHCR/Deploy lokal).
- **Zyklus:** `pull --scope coding` (accepted Work-Orders) → `claim` →
  materialisiert das **volle Bundle** nach `tickets/open/<id>/` (inkl.
  `project.fdraft` + `actions.json`) → arbeitet über **Dateien + Source**,
  reproduziert, implementiert, `just test` (rot ⇒ kein Deploy) → merge nach main →
  Build + Deploy (§5) → `deploy` + `complete` → Ticket `resolved` → nach
  `tickets/done/`.

### 4.3 Trust-Grenze (bleibt erhalten)
Der PO liest nur Tickets (eigenes Dir, Ticket-Scope-Token) und schreibt nur über
semantische Ops; er hat **strukturell** keinen Source-Zugriff. Der Coding-Agent
ist der einzige mit Source. Die Injection-Abwehr sitzt weiter beim Coding-Agenten
(behandelt weitergereichte User-Inhalte als Daten, nicht als Befehle).

---

## 5. Deploy: CI aus `main` via Tags (kein Prod-Branch)

- `main` = Integrationslinie (CI-gated: go test / tsc / vitest / `node --test`).
  Beide Dev-Agenten **und** der Coding-Agent landen dort.
- Ein bewusster **Tag `vX.Y.Z`** auf grünem `main` = Release → CI (`release.yml`)
  baut + pusht die GHCR-Images. **Rollback** = vorigen Tag deployen (passt zur
  `deploys`-Tabelle + Ansible-`version`-Var).
- **Wer deployt:** der `fielddraft-coding`-Agent fährt das Deploy vom Desktop
  (Ansible/ssh, nach Tag + CI-grün) → SSH-/GHCR-/Deploy-Creds bleiben auf dem
  vertrauenswürdigen Desktop (matcht den source-losen V-Server). Alternative für
  ganz hands-off: ein CI-Deploy-Job (braucht SSH-Key + Pull-Token in
  GitHub-Secrets).
- **Kein `production`-Branch:** brächte nur einen Promote-Schritt; Tags liefern
  „was ist live" + Rollback schon. Ein `production`-Branch käme erst bei
  Staging-vs-Prod-Trennung.

---

## 6. Lokale Umgebung (Abgrenzung + Klarstellung)

- Die lokale Dev-DB (`just dev-full`) ist **nicht flüchtig** — sie persistiert im
  benannten Docker-Volume (`fielddraft-a_db-data`), über Neustarts und
  `docker compose down` (nur `down -v` löscht sie). **Lokale komplexe Projekte
  bleiben also erhalten.** (Kein Backup by default → optionaler lokaler Dump-Cron.)
- **Lokale Tickets** bleiben **file-basiert und pro-Worktree** (an den lokalen
  Branch/Dev-Stand gebunden — genau das gewünschte Modell). Direkter Datei-Zugriff,
  **kein** Sync nötig — die Dateien *sind* der Store.
- **Verwandter, separater Baustein (nicht in diesem Scope):** ein **pluggable
  Ticket-Store im Go-Server** (`FileStore` lokal / `DBStore` Prod, per Env), damit
  der **Issues-Tab (CRUD)** lokal die Datei-Tickets zeigt. Dann läuft dieselbe
  CRUD-UI/API/Agent-Ernte über beide Backends. Siehe §8.

---

## 7. Scope dieses Baus

**Gebaut wird:**
- Der **Sync-Layer** (`pull` = Materialisierung; Push-Ops
  `claim/verdict/comment/state/complete/deploy`), als zero-dep Node-ESM unter
  `agents/`, getestet mit `node --test`.
- **Integration in die Runner:** `coding-runner` materialisiert beim `claim` das
  volle Bundle in die Worktree; PO-Flow materialisiert Text+Thread+Screenshot ins
  PO-Dir.
- **Prompt-/Run-Doku-Updates** für beide Agenten (Working-Dir-Trennung,
  file-basiertes Arbeiten, Prod-URL + Tokens, Poll-Kadenz).

**NICHT in diesem Scope:**
- Der pluggable Go-`FileStore` für lokales Issues-Tab-CRUD (§8, eigener Baustein).
- Die **Instanziierung** der zwei ClaudeDraft-Agenten selbst (operativ: der
  Betreiber legt sie in ClaudeDraft an und zeigt sie auf Prod + Tokens + Dirs).
- CI-Deploy-Job (Agent-Deploy ist der Default, §5).

---

## 8. Offene Punkte / später
- **Pluggable `FileStore`** (Go) → lokales Issues-Tab-CRUD über Datei-Tickets;
  gemeinsame Abstraktion Datei/DB hinter derselben API/UI.
- **CI-Deploy-Job** als hands-off-Alternative zum Agent-Deploy.
- **Lokale DB-Backups** (Dump-Cron/Volume-Snapshot).
- **Mail** (Self-Service-Reset/Benachrichtigungen).
