# FieldDraft — Business- und Strategiepapier

![image](assets/logo.png)

> © 2026 Sebastian Leonardo Baltes, Deutschland. Alle Rechte vorbehalten.
> Dieses Dokument einschließlich aller Inhalte, Konzepte und technischen Beschreibungen ist urheberrechtlich geschützt.
> Jede Vervielfältigung, Verbreitung, öffentliche Zugänglichmachung oder sonstige Verwertung – ganz oder teilweise – bedarf
> der vorherigen schriftlichen Zustimmung des Rechteinhabers. Vertraulich – nicht zur Weitergabe bestimmt.

Dieses Dokument ist ein Begleiter zur `README.md`. Die README ist die technische Architektur-Spezifikation und beschreibt *wie* FieldDraft gebaut wird. Dieses Dokument beschreibt *warum, für wen und unter welchen kommerziellen Rahmenbedingungen* — Delivery-Modell, Lizenzierung, Zielgruppen, USPs, IP-Positionierung für den kommerziellen Release, Preis-Überlegungen und Distribution.

**Status:** Arbeitsdokument, nicht final. Konkrete Zahlen, Lizenz-Entscheidungen und Positionierungs-Ansagen sind Diskussionsstände, keine Festlegungen. Vor einer kommerziellen Veröffentlichung wird anwaltlicher Rat eingeholt. Dieses Dokument ist **keine Rechtsberatung**.

---

## 1. Delivery-Modell: Local-First mit optionalem Cloud-Mehrwert

FieldDraft läuft als reines lokales Tool im Browser, ohne Account, ohne Server, ohne Netzwerk. Das MVP tut alles, was der User zum Modellieren braucht; der Backend-Layer (README §4.7) ist eine **optionale Schicht** darüber, nicht Bestandteil der Grundfunktionalität.

Das entspricht strukturell dem **Obsidian-Modell** (Notiz-App mit optionalem Sync-Abo): Die App ist kostenlos und vollständig offline nutzbar, die Cloud-Dienste (Sync, Backup, Collaboration, öffentliche Galerie) sind ein zahlungspflichtiger Mehrwert. Damit unterscheidet sich FieldDraft explizit von Onshape Free, wo Cloud-Speicherung Pflicht und IP-Vorveröffentlichung der Preis dafür ist.

Für die Zielgruppe (Maker, Hardware-Startups unter Linux, Robotik-Hobbyisten) ist das ideal: keine IP-Vorveröffentlichung, keine Abhängigkeit von einem Drittanbieter-Server, trotzdem Bequemlichkeit bei Bedarf.

Mit dem Tauri-Escape-Hatch (README §8) wird aus dem lokalen Browser-Tool eine vollwertige native Desktop-App, ohne dass die Architektur oder die User-Erfahrung bricht.

### 1.1 Die entscheidende Qualitätseigenschaft: „Privat" heißt lokal

In der CAD-Welt ist die Kombination „Browser-basiertes Tool, das lokal funktioniert und bei Bedarf Cloud-Dienste dazu schaltet" **praktisch nicht existent**. Die Konkurrenten sind hart in einer der beiden Richtungen: Onshape, TinkerCAD und Fusion 360 sind Cloud-zentrierte SaaS-Tools; FreeCAD, Plasticity, SolidWorks, Alibre, Solid Edge, BricsCAD und Shapr3D sind Desktop-Apps. Das Modell „lokal + optional Cloud" ist in anderen Sparten etabliert (Obsidian, Logseq, Joplin, Passwort-Manager), aber in CAD hat es bisher niemand konsequent umgesetzt, weil Onshape und Fusion den Cloud-Zwang als Preis für die gute UX normalisiert haben.

**FieldDraft bricht mit dieser Normalität — und die Qualitätseigenschaft, die daraus folgt, ist der strategische Kern des Produkts:**

> „Privat" bedeutet in FieldDraft **nicht** „verschlüsselt auf unseren Servern liegend". Es bedeutet **„niemals auf einem Server gelandet, den du nicht selbst kontrollierst"**.

Das ist qualitativ anders als bei allen „private projects"-Tiers der etablierten SaaS-CAD-Tools. Diese sagen alle dasselbe: „Deine Daten liegen verschlüsselt bei uns, vertraue uns." FieldDraft sagt: „Deine Daten liegen bei dir, du musst niemandem vertrauen." Der Unterschied wird für Hardware-Startups mit Patent-Ambitionen an vier Punkten konkret:

1. **Keine Vertrauens-Annahme über den Anbieter.** Selbst wenn FieldDraft als Firma pleite geht, gehackt wird, von einem Konkurrenten aufgekauft wird oder unter eine gerichtliche Anordnung gerät — private Projekte der User sind davon **nicht betroffen**, weil sie nie verfügbar waren. Bei einem SaaS-Konkurrenten mit „private projects"-Tier wäre jede dieser Situationen ein potenzieller IP-Leak.
2. **Keine Vertrauens-Annahme über die Verschlüsselungs-Implementierung.** Cloud-seitige Verschlüsselung schützt nur, solange der Anbieter die Schlüssel sauber getrennt hält und keine Hintertüren einbaut. Das sind Annahmen, die der User nicht prüfen kann. Bei lokaler Speicherung existiert das Problem nicht — die Datei liegt auf dem eigenen Rechner, der User hat volle Kontrolle.
3. **Keine Exposition gegenüber Subpoenas oder behördlichen Zugriffsforderungen.** Ein US-amerikanischer SaaS-Anbieter kann durch Subpoena oder FISA-Anordnung gezwungen werden, Kundendaten herauszugeben. Ein lokales Tool hat diese Angriffsfläche nicht, weil es keine zentral verfügbaren Daten gibt, die herausgegeben werden könnten.
4. **Keine Telemetrie oder Metadaten-Leaks als Seitenkanal.** Selbst wenn der Projekt-Inhalt verschlüsselt ist, können SaaS-Anbieter Metadaten sammeln (Wann wurde was bearbeitet? Welche Features werden genutzt? Welche Kollaborateure?). Das gibt indirekte Einblicke in Patent-Strategien. FieldDraft sammelt im Local-First-Modus schlicht nichts, weil es keinen Server gibt, der etwas sammeln könnte.

Der Tauri-Escape-Hatch verstärkt dieses Argument zusätzlich: Wer selbst dem Browser-Modell misstraut (kompromittierte CA, DNS-Vergiftung, browser-seitige Telemetrie durch den Browser-Hersteller), bekommt später dieselbe App als native Binary ausgeliefert. Die Vertrauens-Kette wird dabei auf das Minimum verkürzt — Code auf dem eigenen Rechner, kein Netzwerk, kein Drittanbieter dazwischen.

Diese Eigenschaft ist der Grund, warum FieldDraft für die Zielgruppe „Hardware-Startup mit Patent-Ambitionen" nicht nur „eine weitere CAD-Option" ist, sondern eine **strukturell andere Lösung** für ein Problem, das die etablierten SaaS-CAD-Tools nicht lösen können, ohne ihr eigenes Geschäftsmodell aufzugeben.

---

## 2. Zielgruppen und Marktpositionierung

Die ehrliche Frage für jedes neue Tool lautet: Wer sind die ersten 100 Nutzer, und warum wechseln sie von ihrem aktuellen Tool zu FieldDraft?

### 2.1 Primäre Zielgruppe: FreeCAD-Wechsler

Leute, die FreeCAD frustriert nutzen, weil es die einzige kostenlose Linux-Option ist, und die für eine bessere UX sofort wechseln würden. Das Argument **„Onshape-UX mit FreeCAD-Datenhoheit"** trifft diese Gruppe direkt. Hebel:

- STL-Export funktioniert vom ersten Tag.
- Der Sketch-Constraint-Solver (PlaneGCS mit eigener Adapter-Schicht, siehe README §4.4) fühlt sich besser an als FreeCADs Sketcher — insbesondere Drag-Stabilität ohne Topologie-Kippen und Auto-Inferencing beim Zeichnen.
- Das Tool ist im Browser ohne Installation nutzbar und funktioniert offline.
- Die SDF-basierte Boolean-Robustheit eliminiert die TNP-Schmerzen, mit denen FreeCAD-User chronisch kämpfen.

### 2.2 Sekundäre Zielgruppe: Onshape-Free-Nutzer mit Patent-Ambitionen

Leute, die Onshape mögen, aber nervös werden, sobald sie an IP-Schutz denken. Für die ist Datenhoheit das Kaufargument: Projekte liegen standardmäßig lokal im Browser, werden nie zwangs-veröffentlicht, und können auf Wunsch (aber niemals unter Zwang) gesynct werden.

### 2.3 Tertiäre Zielgruppe: Maker mit Einsteiger-Bedürfnissen

Hobbyisten, die bislang TinkerCAD nutzen, weil Onshape und FreeCAD für sie zu mächtig sind, und die bei Bedarf eines Tages zu „richtigem" CAD wachsen wollen. Das adressiert der geplante Quick Mode (siehe Abschnitt 3.4 unten): Einsteiger-UX und Profi-UX auf derselben Codebase, ein Wachstums-Pfad ohne Tool-Wechsel.

### 2.4 Explizit nicht die erste Zielgruppe

Professionelle Ingenieure in Unternehmen mit SolidWorks/Onshape-Standard-Budgets. Sie brauchen STEP-Export, normgerechte 2D-Zeichnungen, ISO-Passungen und vollständige Assemblies — alles Dinge, die FieldDraft im MVP bewusst nicht hat (siehe README §12 Harte Architektur-Grenzen). FieldDraft ist **kein** Ersatz für den klassischen Maschinenbau-CAD-Stack, und der Versuch, diese Gruppe zu erobern, wäre Selbstbetrug.

### 2.5 Distributionskanäle

Der wichtigste Verbreitungs-Kanal für Maker-Tools ist **YouTube und Reddit** (r/3Dprinting, r/functionalprint, r/cad). Ein überzeugendes „Ich habe ein Gehäuse in FieldDraft in 5 Minuten gebaut und direkt gedruckt"-Video ist wertvoller als jede konventionelle Marketingmaßnahme.

Der „Playable Demo"-Meilenstein nach K7 (Feature-Tree-UI, Booleans, Export) ist der strategisch wichtigste Schnitt im ganzen Projekt — das ist der Punkt, an dem das erste öffentliche Video gedreht werden kann, weil der vollständige Sketch→Extrude→Boolean→Export-Workflow dann zum ersten Mal nachvollziehbar ist.

---

## 3. USPs — was die Architektur ermöglicht, das andere nicht haben

### 3.1 Linux-nativ im Browser

Keine Installation, keine Cloud-Pflicht, kein OS-Lock-in. Fehlt bei SolidWorks, Fusion 360, Solid Edge, Alibre, Plasticity. FreeCAD deckt Linux ab, aber nicht „im Browser ohne Installation".

### 3.2 Technische Privatheit statt vertrauensbasierter Privatheit (Onshape-UX mit echter Datenhoheit)

Die UX-Referenz ist Onshape (siehe README §6 und `onshape-ux.md`), die Datenhaltung ist lokal und standardmäßig privat. Der entscheidende Unterschied zu allen „private projects"-Tiers der etablierten SaaS-CAD-Tools ist ein qualitativer, kein gradueller: **FieldDraft verlangt keine Vertrauens-Annahme über den Anbieter**, weil private Projekte nie auf einen Server gelangen, den der User nicht selbst kontrolliert. Details und die vier konkreten Konsequenzen (kein Firmen-Exit-Risiko, keine Verschlüsselungs-Backdoor-Sorge, keine Subpoena-Exposition, keine Metadaten-Leaks) stehen in §1.1. Zusammen mit dem Tauri-Escape-Hatch ist das eine strukturell andere Privatheits-Zusage als alles, was Onshape, Fusion, SolidWorks-Cloud oder TinkerCAD anbieten können, ohne ihr eigenes Geschäftsmodell aufzugeben — und gleichzeitig bleibt die UX auf Onshape-Niveau. Für Hardware-Startups mit Patent-Ambitionen ist das der eigentliche Kauf-Grund, und für die Positionierung im Markt (siehe §2.2) der Schlüssel-Hebel.

### 3.3 Native SDF-Superkräfte

Gyroid/TPMS-Infill, Procedural Noise/Displacement, Domain Deformation (Bend/Twist), Morph — als native Features, nicht als Post-Processing-Tricks. Siehe README §4.1, §6.1 und Roadmap K11. Das ist für den 3D-Druck-Kontext ein echtes Alleinstellungsmerkmal gegenüber klassischen B-Rep-Tools, in denen diese Strukturen entweder nicht existieren oder nur mit irrsinnigem Rechenaufwand darstellbar sind.

### 3.4 Quick Mode (Einsteiger-UX) und Pro Mode auf derselben Codebase

Die Feature-as-Function-Architektur (README §4.6) erlaubt, dass ein „QuickCube" einfach ein Feature mit vereinfachtem Schema ist, das intern per `execute` einen regulären Sketch+Extrude-Baum komponiert. Im Quick Mode zeigt der FeatureDialog drei Schieberegler; im Pro Mode sieht der User denselben Baum darunter, aber mit dem vollen Extrude-Dialog. **Es gibt keine zwei Codebases**, nur zwei Schema-Präsentationen über derselben Registry.

Damit deckt FieldDraft Einsteiger (wie TinkerCAD) und Profis (wie Onshape) mit einer Codebase ab und bietet einen nahtlosen Wachstums-Pfad ohne Tool-Wechsel. Das löst ein klassisches Problem: Einsteiger-CAD-Tools und Profi-CAD-Tools sind normalerweise getrennte Welten; wer bei TinkerCAD anfängt, muss irgendwann das Tool wechseln, um zu lernen, was ein Feature-Tree ist. In FieldDraft ist der Feature-Tree von Anfang an da, nur unsichtbar.

Das ist als Phase-3-Feature geplant und setzt K7 (Pro-Mode mit Feature-Tree-UI) voraus. Der Quick Mode wird oben drauf gebaut, nicht umgekehrt, weil er die Pro-Mode-Infrastruktur konsumiert, nicht umgekehrt.

### 3.5 KI-Integration mit parametrischem Output

Die JS-API aus README §4.6 ist eine öffentliche, skriptbare Schnittstelle über einer deterministischen Feature-Registry — genau die Form, die ein LLM direkt konsumieren und erzeugen kann.

Prompt: *„Mach mir ein Gehäuse für einen Raspberry Pi 4 mit Lüftungsschlitzen."*
Output: eine Sequenz von `fd.sketch()`, `fd.extrude()`, `fd.pattern()`, `fd.fillet()`-Aufrufen, die als JS-Feature registriert und ausgeführt werden. Das Ergebnis ist **kein Mesh-Blob, sondern ein vollständig parametrischer Feature-Tree** — der User kann jeden Parameter ändern, Maße anpassen, Features löschen oder umordnen, so wie bei jedem handgebauten Modell.

Bestehende „KI-CAD"-Tools liefern opake Meshes, die nach der Generierung nicht mehr bearbeitbar sind. FieldDraft liefert editierbare Modelle. Das ist ein Alleinstellungsmerkmal, das als Nebeneffekt der Feature-as-Function-Architektur abfällt, nicht als separat zu bauendes KI-Modul.

Konkrete Einstiegs-Features für eine spätere Phase:

- **Chat-Panel im UI** mit LLM-API-Anbindung (Anthropic, OpenAI), das Prompts entgegennimmt und JS-Feature-Calls generiert.
- **„Explain this model"** — Feature-Tree wird in natürliche Sprache übersetzt.
- **„Fix my sketch"** — der User markiert einen roten (über-constrainten) Sketch, die KI analysiert die Constraint-Konflikte und schlägt Fixes vor.

Die Implementierung ist kein MVP-Thema. Architektonisch ist sie vollständig vorbereitet.

### 3.6 Direct Digital Manufacturing-Fokus

Additive Fertigung (FDM, SLA, SLS), SDF-direktes CAM ohne B-Rep-Zwischenformat, Dentaltechnik-ähnliche Workflows. Siehe README §12. Das ist die strategische Alternative zum Maschinenbau-CAD-Stack, den FieldDraft bewusst nicht bedient.

### 3.7 Schwesterprojekt BeamDraft teilt Solver und Adapter-Schicht

Ein investierter Entwicklungs-Euro dient zwei Produkten. Siehe README §4.4 und §14. Das ist ein struktureller Kosten-Vorteil, der in keinem Vergleichsprojekt so existiert.

---

## 4. Lizenzierungs-Optionen

Die Lizenz-Entscheidung ist bewusst noch offen. Drei gangbare Pfade, nicht gegenseitig ausschließend, alle architektonisch verträglich mit dem aktuellen Code (insbesondere mit der PlaneGCS-LGPL-Randbedingung aus README §4.4):

### 4.1 Open-Source-Kern + kommerzielle Cloud-Dienste („GitLab-Modell")

Kernel, Solver, Frontend unter einer Copyleft-Lizenz (AGPL oder GPL), damit SaaS-Konkurrenten denselben Code nicht einfach hosten können, ohne ihre Änderungen zurückzugeben. Cloud-Dienste (Sync, Collaboration, Galerie, Managed Hosting) kommerziell.

- **Vorteile:** Community-Beiträge, Vertrauen bei der Zielgruppe (Maker und Startups lieben OSS), AGPL verhindert Closed-Source-Re-Hosting strukturell.
- **Nachteile:** Eigener Code lässt sich später nicht mehr proprietär relizenzieren, sobald externe Contributor drin sind — es sei denn, man verlangt einen Contributor License Agreement (CLA) von jedem Beitragenden.
- **Kompatibilität:** AGPL ist mit PlaneGCS (LGPL-2.1+) vereinbar. BeamDraft teilt den Solver, was unter OSS-Modellen unproblematisch ist.

### 4.2 Source-Available + Fair-Use („BSL/SSPL-Modell")

Code einsehbar und persönlich/für kleine Teams kostenlos nutzbar, aber kommerzielle Hosting-Dienste (jemand anderes betreibt FieldDraft als SaaS) erfordern eine kommerzielle Lizenz. Der Weg, den MongoDB (SSPL) und HashiCorp (BSL) gewählt haben.

- **Vorteile:** Volle Kontrolle über kommerzielle Nutzung. Möglichkeit, nach einer Karenzzeit (z. B. 4 Jahre) auf echte OSS umzusteigen — BSL erlaubt das als Standard-Mechanismus.
- **Nachteile:** Weniger Community-Enthusiasmus als bei echter OSS. Die Maker-Zielgruppe misstraut „Source-Available"-Lizenzen und bevorzugt eindeutige OSS-Modelle.

### 4.3 Freemium mit Feature-Gates („Figma-Modell")

Kernprodukt kostenlos für Einzelnutzer mit lokalem Speicher. Bezahlfeatures: mehr als N Projekte, Team-Collaboration, Priority-Export (STEP), Custom-Feature-Editor, erweiterte Simulation.

- **Vorteile:** Niedrige Einstiegshürde.
- **Nachteile:** Feature-Identifikation schwierig, ohne das Kernprodukt zu verkrüppeln. Läuft dem „Onshape-UX mit Datenhoheit"-Versprechen zuwider, wenn wichtige Features hinter einer Paywall verschwinden.

### 4.4 Arbeitshypothese

Wahrscheinlich **Open-Source-Kern + kommerzielle Cloud-Dienste** als Startmodell: schützt vor Closed-Source-Re-Hosting, ist mit PlaneGCS kompatibel, kann später auf BSL wechseln, wenn der Markt das verlangt — Redis, Elastic und HashiCorp haben vorgemacht, dass ein solcher Wechsel möglich ist. Die tatsächliche Entscheidung fällt vor einem kommerziellen Release.

**Die Entscheidung ist explizit noch offen.** Diese Sektion ist Diskussionsstand, keine Festlegung.

---

## 5. Preis-Überlegungen

Konkrete Preispunkte sind nicht final und dienen ausschließlich als Diskussionsgrundlage:

### 5.1 Mögliche Tier-Struktur

- **SaaS-Free mit öffentlichen Projekten.** Spiegelt das Onshape-Free-Modell, aber als ehrliche Ansage kommuniziert: „Projekte sind öffentlich, dafür zahlst du nichts." Keine versteckten Bedingungen im Kleingedruckten. Für Maker, die ihre Arbeit ohnehin in der Galerie zeigen wollen, ist das ein fairer Deal.
- **SaaS-Paid mit privaten Projekten und Sync.** ~5 €/Monat als Einstiegspunkt. Für Hardware-Startups mit IP-Schutz-Bedarf.
- **Lokale Version als Einmalkauf.** ~120 € für die aktuelle Major-Version inklusive aller Minor-Updates. Nach dem JetBrains-Modell: Version 1.x ist inklusive, Version 2.0 ist ein neuer Kauf mit Upgrade-Rabatt. Das vermeidet die „lebenslange Updates"-Falle, in die viele Indie-Tools (Sublime Text vor dem Abo-Umstieg, Sketch) geraten sind, und bleibt fair für beide Seiten: Der Käufer bekommt eine lange stabile Version, der Entwickler hat einen nachhaltigen Einkommensstrom.
- **Lokale Version als Abo-Alternative.** ~5 €/Monat für Nutzer, die den Einmalpreis scheuen oder nur kurzfristig brauchen.

### 5.2 Preispositionierung

Die Preispunkte sind **aggressiv günstig** im Vergleich zur Konkurrenz:

- Plasticity: ~149 USD/Jahr.
- Onshape Standard: ~1.500 €/Jahr.
- SolidWorks: mehrere tausend Euro pro Jahr.
- Fusion 360 Personal: kostenlos, aber mit Nutzungs-Einschränkungen und ohne Linux-Support.

Für die Maker-Zielgruppe mit schmalem Budget ist **120 € einmalig** ein Impulskauf-Preispunkt. Das ist die Positionierung: nicht als Konkurrenz zu SolidWorks, sondern als nachhaltige Indie-Tool-Option, die sich ein Hobbyist oder ein Startup-Gründer ohne Genehmigung vom CFO leisten kann.

### 5.3 Nachhaltigkeit

Die „lebenslange Updates"-Zusage ist tragbar, solange die zahlende Nutzerbasis wächst. Wenn sie stagniert, wird es eng — Sublime Text und andere Indie-Tools sind in diese Falle gelaufen. Der JetBrains-artige Major-Version-Cut ist der saubere Ausweg: Nach zwei oder drei Jahren kommt Version 2.0 als neuer Kauf mit Upgrade-Rabatt für Bestandskunden.

**Nichts davon ist festgelegt.** Das sind Diskussionsstände, die vor einem kommerziellen Release neu bewertet werden müssen.

---

## 6. IP-Position für den kommerziellen Release

Der **agent-relevante Teil** der IP-Position (keine Onshape-Markenbegriffe in API/Produktnamen, kein Quellcode-Kopieren aus Dritt-Kernels, Konzepte sind Stand der Technik, originäre FieldDraft-Architektur) ist in README §8.1 dokumentiert und dort eine harte Vorgabe für den Implementierer. Dieser Abschnitt beschreibt die **zusätzlichen Schritte**, die vor einem kommerziellen Release erforderlich sind.

### 6.1 Patent-Recherche

Vor einer kommerziellen Veröffentlichung wird eine professionelle Patent-Recherche durchgeführt, speziell zu PTC/Onshape-Patenten im Bereich Cloud-CAD und parametrischem Modeling. Das kostet ein paar hundert Euro über einen spezialisierten IP-Anwalt und gibt Rechtssicherheit. Die Recherche ist nicht Teil des MVP-Scopes — im Hobby- und Experimental-Stadium ist sie nicht erforderlich — aber sie ist als Pflicht-Schritt vor dem Release fest eingeplant.

### 6.2 Marken-Recherche und -Anmeldung

Aktuell gibt es keine bekannten Markeneintragungen für „FieldDraft" oder „BeamDraft" (siehe README §15 Domains und Markenlage). Vor einem kommerziellen Release wird eine Marken-Recherche in den relevanten Jurisdiktionen (EU, USA, DE) durchgeführt, und bei Bedarf wird die Marke angemeldet.

### 6.3 Lizenz-Prüfung aller Drittbibliotheken

Die README §8 OSS-Readiness-Disziplin verlangt, dass der Agent jede neue Drittbibliothek dokumentiert (in `THIRD_PARTY.md` oder als Cargo.toml-Kommentar). Vor einem Release erfolgt eine Gesamt-Prüfung, dass keine inkompatible Lizenz im Baum steckt — insbesondere keine GPL-only-Crates, falls das gewählte Lizenzmodell Source-Available ist, und keine Secrets oder proprietären Teile, falls das gewählte Modell AGPL ist.

### 6.4 Anwaltlicher Rat

Vor dem Release wird anwaltlicher Rat eingeholt zu:

- Lizenzmodell-Finalisierung.
- Allgemeinen Geschäftsbedingungen.
- Datenschutz-Anforderungen (falls SaaS-Komponente kommt — DSGVO-Compliance).
- Marken- und Patent-Position.

### 6.5 Disclaimer

Die IP-Einschätzungen in diesem Dokument und in README §8.1 sind eine technische Einschätzung auf Basis der Architektur und der recherchierten Sachlage. Sie sind **keine Rechtsberatung**. Der Autor dieses Dokuments ist kein Anwalt. Vor einer kommerziellen Veröffentlichung ist anwaltlicher Rat zwingend.

---

## 7. Open-Source-Fallback-Strategie

Falls das kommerzielle Modell nicht fliegt — weil der Markt zu klein ist, weil die Konkurrenz schneller ist, weil das Preis-/Feature-Verhältnis nicht trifft — hat die Arbeit trotzdem Wert:

- Als **Open-Source-Projekt** für die Maker-Community. FieldDraft deckt eine Lücke (Linux-natives, browser-basiertes, lokal-first CAD mit Onshape-UX), die auch als reines OSS-Projekt einen klaren Nutzen hat.
- Als **Portfolio-Stück** für den Entwickler — die technische Tiefe der Architektur (SDF-Kernel mit Provenance, Ghost Topology, Feature-as-Function mit Dual-Backend, Mesh-to-SDF-Unification von Sweep und Import) ist für sich genommen bemerkenswert.
- Als **technische Grundlage für BeamDraft**, das Schwesterprojekt, das Solver und Adapter-Schicht teilt und das unabhängig von der FieldDraft-Kommerzialisierung vorangetrieben werden kann.

Wichtig: Der Wechsel zu Open Source ist **keine Notlösung, sondern eine Entscheidung**. Die OSS-Readiness-Disziplin aus README §8 stellt sicher, dass der Wechsel zu jedem Zeitpunkt möglich ist, ohne Umbau des Codes. Es gibt keine proprietären Abhängigkeiten, keine Secrets im Repository, keine Architektur-Entscheidungen, die nur unter einer kommerziellen Lizenz Sinn machen. Jedes Architektur-Konzept (Feature-Registry, Query-System, Spatial-Mask-Fillets, Ghost Topology) ist unter OSS und unter kommerziellen Lizenzen gleichermaßen sinnvoll.

---

## 8. Roadmap-Reihenfolge für die Kommerzialisierung

Die technische Roadmap in README §10 ist die Grundlage; die kommerzielle Roadmap baut darauf auf:

1. **K1 bis K9** — technischer MVP, rein Entwicklungsarbeit.
2. **Nach K7 ist der „Playable Demo"-Schnitt** — der erste Moment, an dem ein öffentliches Video gedreht werden kann, das den vollständigen Sketch→Extrude→Boolean→Export-Workflow zeigt. Das ist der strategisch wichtigste Marketing-Moment im Projekt. Ein gutes Video hier kann die Zielgruppe (Maker auf YouTube/Reddit) organisch erreichen.
3. **K10 bis K13** — Feature-Abrundung (Fillets/Chamfers, SDF-Superkräfte, Gewinde, Mesh-to-SDF).
4. **M11 bis M13** — Backend, Auth, Sync, öffentliche Galerie. Erst jetzt ist das SaaS-Modell technisch umsetzbar.
5. **Phase 3 (P1 bis P5)** — Assemblies, FK/IK, R3F-Export, physikalische Abfragen, FEM. Das sind alle Features, die die Differenzierung gegenüber klassischen CAD-Tools schärfen.
6. **Quick Mode und KI-Integration** (als Phase-3-Follow-ups) — die Einsteiger-Tier und der LLM-Use-Case, die beide auf der Feature-as-Function-Architektur aufsetzen.
7. **Patent-Recherche, Marken-Anmeldung, anwaltliche Prüfung** — als Abschluss-Schritte vor dem kommerziellen Launch.
8. **Launch** — mit klarer Preis- und Lizenz-Struktur, Video-Demo, Galerie-Showcase, Reddit-/YouTube-Start.

Diese Reihenfolge ist bewusst so geschnitten, dass der Druck erst spät kommt. Bis K7 kann in Ruhe gebaut werden, ohne dass Marketing-Fragen das Design verbiegen. Erst nach dem Playable-Demo wird die kommerzielle Seite relevant.

---

## 9. Verwandte Dokumente

- **`README.md`** — technische Architektur-Spezifikation. *Wie* das Tool gebaut wird. Die §8 dort enthält die agent-relevanten Vorgaben zu OSS-Readiness und IP-Markennamen, die beim Implementieren zwingend einzuhalten sind.
- **`onshape-ux.md`** — UX-Referenz, gegen die FieldDraft den Sketch- und Feature-Editor implementiert.
- **`BUSINESS.md`** (dieses Dokument) — *Warum, für wen, unter welchen kommerziellen Rahmen.* Arbeitsdokument, ändert sich bis zum Release.
