# AUTH-CONTRACT — geteilter Auth-API-Vertrag (fielddraft ↔ newpulous)

SSOT für den HTTP-Vertrag der Anmelde-/Konten-Endpoints, die mehrere Apps des
Betreibers teilen. Geteilt wird **der Vertrag, nicht die Implementierung**:
fielddraft (Go + MariaDB + React) und newpulous (Node/ESM + JSON-Store,
framework-los) implementieren ihn jeweils selbst und testen gegen dieses
Dokument. Abgestimmt 2026-09-10 zwischen den Agenten `fielddraft` und
`newpulous`; Änderungen hier bitte dem jeweils anderen melden.

## Konventionen

- Alle Bodies sind JSON. Fehler haben immer die Form `{"error": "<meldung>"}`
  mit passendem HTTP-Status.
- Die Sitzung ist ein **opaker Zufallswert** (mindestens 128 Bit aus einem
  CSPRNG), **serverseitig widerrufbar**. Der Transport ist
  implementierungsfrei: Cookie **oder** `Authorization: Bearer <token>`.
  fielddraft: Cookie `session` mit `HttpOnly`, `SameSite=Lax`, `Secure` (in
  Prod), `Path=/`, 7 Tage. newpulous: Bearer-Token (32 Bytes hex, 30 Tage) —
  ein Cookie ist dort unmöglich, weil die API mit
  `Access-Control-Allow-Origin: *` antwortet und CORS bei Wildcard das
  Mitsenden von Anmeldedaten verbietet (Seite wird aus wechselnden Herkünften
  geladen).
- User-Objekte in Antworten enthalten **nie** Passwort-Ableitungen oder Salts.
- Feldnamen sind `snake_case`.

## Nicht-Verrats-Regeln (verbindlich)

1. **Login:** unbekannte E-Mail und falsches Passwort liefern denselben
   Status (401) mit derselben Meldung (`invalid credentials`).
2. **Reset-Anforderung:** antwortet **immer 200** `{"status":"ok"}`, egal ob
   die E-Mail zu einem Konto gehört.
3. **Invite-Info:** alles außer einem offenen (unbenutzten, nicht
   abgelaufenen) Invite liefert nur `{"valid": false}` — kein Unterschied
   zwischen „gibt es nicht", „schon benutzt", „abgelaufen".

## Passwort-Politik

Minimal **8 Zeichen**, keine weiteren Kompositionsregeln. In jeder
Implementierung an **genau einer Stelle** deklariert und an **allen** drei
Eintrittspunkten durchgesetzt: Registrierung, Reset, Änderung. Verstoß → 400.

Die Passwort-Ableitung ist implementierungsfrei, muss aber ein memory-hardes
Verfahren sein (fielddraft: Argon2id; newpulous: scrypt N=2^15).

## Endpoints

### POST /api/auth/register

Registrierung ist **invite-gated**. Request:
`{"email", "display_name", "password", "invite"}` — `invite` alternativ als
Query `?invite=<token>`. Passwort ist Pflicht. Die **Login-Kennung ist
App-Sache**: bei fielddraft die E-Mail (Pflicht), bei newpulous der Name
(E-Mail optional, dort entsteht das Erstkonto aus der Dienst-Umgebung ohne
Adresse; ob sie Pflicht wird, entscheidet der Betreiber — offen). Verbindlich
ist nur: **wenn eine E-Mail gesetzt ist, ist sie eindeutig pro Konto**, und
der Passwort-Reset setzt eine hinterlegte E-Mail voraus (Konto ohne Adresse →
kein Reset; eine Route zum Nachtragen ist erlaubt).

Antworten: 200 mit dem User-Objekt + gesetztem Session-Cookie (direkt
eingeloggt) · 400 fehlende Felder / Passwort zu kurz · 403 fehlender oder
ungültiger/verbrauchter/abgelaufener Invite · 409 E-Mail schon vergeben.

Invite-Validierung + -Verbrauch + Konto-Anlage müssen **atomar** sein: zwei
Registrierungen mit demselben Token dürfen nie beide durchgehen, und eine
Duplikat-E-Mail darf den Token nicht verbrennen.

### POST /api/auth/login

Request: Login-Kennung + Passwort → 200 User-Objekt + Sitzung, sonst 401
(siehe Nicht-Verrats-Regel 1). Gesperrte Konten: ebenfalls 401. Kanonische
Felder `{"email", "password"}` (fielddraft); newpulous nimmt
`{"name", "passwort"}` — `email` ist dort im Login-Body **nicht** zulässig.

### POST /api/auth/logout

Löscht die Sitzung serverseitig und leert das Cookie. Antwort 200
`{"ok": true}`; auch ohne gültige Sitzung kein Fehler.

### GET /api/auth/me

200 mit dem User-Objekt der aktuellen Sitzung, sonst 401.

### POST /api/auth/password-reset-request

Request `{"email"}` → immer 200 `{"status":"ok"}` (Regel 2). Existiert das
Konto, wird ein Reset-Token erzeugt und per Mail zugestellt (siehe
Mail-Anforderungen). 400 nur bei fehlendem `email`-Feld.

### POST /api/auth/password-reset

Request `{"token", "new_password"}` → 200 `{"status":"ok"}` · 400 bei
fehlenden Feldern, Passwort < 8 Zeichen, oder unbekanntem / verbrauchtem /
abgelaufenem Token (einheitliche Meldung `invalid or expired token`).
Beim Einlösen alle bestehenden Sitzungen des Kontos zu invalidieren ist
erlaubt, aber nicht gefordert.

### POST /api/auth/password-change

Eingeloggtes Passwort-Ändern. Request `{"old_password", "new_password"}` →
200 · 401 ohne Sitzung oder bei falschem `old_password` · 400 bei Passwort
< 8 Zeichen. (Stand 2026-09-10 in keiner der beiden Apps implementiert;
der Name ist hiermit reserviert, damit kein dritter Dialekt entsteht.)

### Invites

„Berechtigt" heißt **berechtigt im Sinne der App**: fielddraft hat Rollen und
bindet die Verwaltung an Admins; newpulous hat bewusst keine Rollen — dort
darf jeder Angemeldete einladen. `GET`-Liste und `DELETE` sind optional
(newpulous baut sie vorerst nicht).

- `POST /api/invites` (berechtigt): Request
  `{"invitee_name", "invitee_email", "message"}` → 200 Invite-Objekt inkl.
  `token`, `expires_at` (Default-TTL 14 Tage) und `register_url`
  (`…/register?invite=<token>`).
- `GET /api/invites` (berechtigt, optional): Liste aller Invites mit Status
  (`open` / `used` / `expired`).
- `DELETE /api/invites/{token}` (berechtigt, optional): offenen Invite
  zurückziehen.
- `GET /api/invites/{token}/info` (**öffentlich**, für das Prefill des
  Registrierungsformulars): offener Invite →
  `{"invitee_name", "invitee_email", "valid": true}`; alles andere →
  `{"valid": false}` (Regel 3).

## Reset-Token

- Opaker Zufallswert, mindestens 128 Bit CSPRNG (fielddraft: 16 Bytes hex,
  32 Zeichen).
- **Einmalig** (Einlösen verbraucht) und **1 h gültig**.
- Gespeichert als **Abdruck** (Hash), nicht im Klartext. fielddraft speichert
  Stand 2026-09-10 noch Klartext — bekannte Abweichung, wird beim nächsten
  Anfassen des Reset-Codes behoben; newpulous baut von Anfang an mit Abdruck.
- Der Token wandert in der Mail sowohl als Link
  (`<PUBLIC_URL>/reset?token=<token>`) als auch als **kopierbarer Code im
  Klartext des Rumpfs**. Grund: Absender ist eine Fremdadresse und Links
  zeigen ggf. auf `sslip.io`-Adressen mit IP im Namen — entwertet ein
  Spamfilter den Link, muss der Code allein reichen. Die Mail bleibt
  deshalb schlicht (Plain-Text, kein HTML nötig).

## Mail-Anforderungen

- Versandweg: **SMTP auf Port 587 mit STARTTLS** — gemessen (devops,
  2026-09): ausgehend sind 25 und 465 dicht, 587 ist offen (Host wie
  Container, gegen Gmail/Brevo/Mailgun/Postmark). Kein API-Dienst nötig.
  **Nicht** implizites TLS auf 465 planen.
- Konfiguration über Env: `SMTP_HOST`, `SMTP_PORT` (587), `SMTP_USER`,
  `SMTP_PASSWORD`, `SMTP_FROM`, `PUBLIC_URL`.
- **Leere SMTP-Werte heißen „Versand aus", kein Startfehler.** Der Dienst
  fällt auf den Ohne-Mail-Weg zurück (Token/Invite-Code anzeigen bzw.
  loggen) und meldet das in einer Logzeile beim Start. Ein fehlender
  Mailweg darf keinen Deploy aufhalten, aber auch nicht still bleiben.
- `SMTP_FROM` wird **unverändert** als Absender verwendet (bei
  Gmail-Zugängen muss es die Kontoadresse selbst sein, sonst weist der
  Server ab oder schreibt um).

## Bekannte Abweichungen (Stand 2026-09-10)

| Punkt | fielddraft | newpulous |
|---|---|---|
| `GET /api/auth/me` | liegt noch auf `GET /api/me`; Migration auf den kanonischen Pfad steht aus | konform |
| Passwort-Minimum bei Registrierung | seit 16a64035 durchgesetzt | konform |
| Reset-Token-Speicherung | Klartext (s. o.) | Abdruck |
| `password-change` | fehlt | war `/api/auth/passwort`; wird umbenannt und übernimmt dabei `old_password`/`new_password` |
| Feldnamen `login`/`register` | konform | deutsch (`name`, `passwort`, …) — bleibt vorerst, Umbenennung ohne Nutzen solange kein Dritter die API spricht |
| E-Mail am Konto | Pflicht + Login-Kennung | optional (eindeutig wenn gesetzt) + Nachtrage-Route; Betreiber-Entscheidung zur Pflicht offen |
| Invite-Pfade | konform | Umzug von `/api/auth/invite` auf `/api/invites` zugesagt; `GET`-Liste und `DELETE` nicht gebaut (optional) |
| Mailversand | fehlt komplett (Token wird geloggt); übernimmt den 587/STARTTLS-Weg, sobald gebaut | in Arbeit; wechselt auf „konform", sobald gegen einen echten Kanal gemessen |
