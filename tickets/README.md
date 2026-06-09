# Tickets

In-App gemeldete Fehler und Feature-Wünsche. Erzeugt über den **Report Issue…**-Button im App-Menü (Burger oben links → *Report Issue…*).

## Ablauf

1. User klickt *Report Issue…*, tippt Titel + Beschreibung. Beim Öffnen wird ein Screenshot des gesamten App-Fensters (Toolbar, Panels und 3D-Viewport) erfasst.
2. Der Dialog schickt ein POST an `/api/tickets`.
   - **Dev (`just dev`):** Das Vite-Plugin `ticketWriter` (`web/vite.config.ts`) legt das Ticket lokal unter `tickets/open/<id>/` ab.
   - **Prod:** Der Go-Server (`server/main.go`) bedient dieselbe Route.
3. Der Agent (ClaudeDraft) arbeitet offene Tickets ab, wenn sonst nichts ansteht.

## Verzeichnis-Layout

    tickets/
      open/                  # offene Tickets (gitignored — lokale Runtime-Artefakte)
        <id>/
          ticket.json        # strukturierte Metadaten (Titel, Version, Zeit, UA, URL, status)
          ticket.md          # menschenlesbare Spiegelung
          project.fdraft     # exaktes Projekt-JSON zum Reproduzieren
          screenshot.png     # gesamtes App-Fenster zum Zeitpunkt der Meldung
      done/                  # abgeschlossene Tickets (gitignored)

`<id>` ist `YYYYMMDD-HHMMSS-<rand>`, also chronologisch sortierbar.

## Für den Agenten: Abarbeitung

Wenn keine andere Aufgabe ansteht, offene Tickets bearbeiten:

1. `tickets/open/` nach `<id>/` durchsehen (ältestes zuerst).
2. `ticket.md` lesen, `screenshot.png` ansehen, `project.fdraft` zum Reproduzieren laden.
3. Fix/Feature umsetzen, testen (`just test`), committen + pushen wie üblich.
4. Ticket-Ordner nach `tickets/done/<id>/` verschieben und in `ticket.json` `status` auf `resolved` setzen; eine kurze `resolution.md` mit Commit-Hash + Zusammenfassung hinzufügen.

Tickets liegen außerhalb von git (siehe `.gitignore`) — sie sind lokale Runtime-Daten, kein Quellcode. Nur diese README ist versioniert.
