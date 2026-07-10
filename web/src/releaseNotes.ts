// AUTO-SEEDED from git history by scripts/genReleaseNotes.mjs, then the
// hand-maintained SSOT for the in-app release notes. Add one entry per version
// bump (newest version first); re-run the seeder only to rebuild from scratch.

export type ReleaseNoteKind = "feat" | "fix" | "perf";

export interface ReleaseNoteEntry {
  /** feat = new capability, fix = bug fix, perf = performance. */
  kind: ReleaseNoteKind;
  /** Short user-facing description (German, mirrors the commit subject). */
  text: string;
  /** Originating subsystem/scope, e.g. "extrude", "sketch". */
  scope?: string;
  /** Originating ticket id(s), if the change came from a Report Issue ticket. */
  tickets?: string[];
}

export interface ReleaseNote {
  /** Semantic version without the leading "v", e.g. "0.6.342". */
  version: string;
  entries: ReleaseNoteEntry[];
}

/** All releases, newest first. */
export const RELEASE_NOTES: ReleaseNote[] = [
  {
    version: "0.6.491",
    entries: [
      {
        kind: "feat",
        scope: "mesher",
        text: "Lichen-Mesher M4: gekrümmte analytische Flächen (Zylinder, Kugel, Kegel) werden jetzt trianguliert — über den periodischen Chart mit Naht-Schnitt (geteilte Steiner-Indizes, unsichtbare Naht) und Pol-Kappen via Ring + Fächer, dieselbe Mechanik wie der Hybrid-PASM-Export. Ein Zylinder oder eine Halbkugel vermascht damit als geschlossenes, wasserdichtes 2-Manifold. Torus (doppelt periodisch) und Gyroid-Flächen (chartlos) bleiben vorerst Punktwolke.",
        tickets: ["10df1b20"],
      },
    ],
  },
  {
    version: "0.6.490",
    entries: [
      {
        kind: "fix",
        scope: "brep",
        text: "Skalierte Kugeln bekommen wieder vollständige BRep-Kanten: Die exakte Schnittkreis-Berechnung (Kugel×Ebene, Kugel×Kugel) ignorierte die Scale-Transform des Features — eine ×0.02 skalierte Kugel wurde mit ihrem unskalierten Radius (50 statt 1) geschnitten, der Seam an Box-Flächen fiel dadurch stillschweigend weg und die betroffene Fläche war nicht mehr komplett von Kanten umschlossen. Nicht-uniform skalierte Kugeln (Ellipsoide) nehmen jetzt den Parameterraum-Scan statt der geschlossenen Form.",
        tickets: ["20260710-163918-a26a"],
      },
    ],
  },
  {
    version: "0.6.489",
    entries: [
      {
        kind: "feat",
        scope: "mesher",
        text: "Debug-Mesher Lichen (Menü Debug ▸ Mesher: Lichen…): direkter SDF+BRep-Oberflächen-Mesher. Punkte wachsen als hexagonale Waben-Front pro BRep-Fläche über die Oberfläche — mit einer HARTEN Kanten-Wand: kein Punkt überspringt je eine BRep-Kante auf eine Nachbarfläche oder ins Leere, und der U(p)-Anzeigekreis bleibt stets innerhalb der Fläche. Die Triangulierung erfolgt pro Fläche über den robusten Constrained-Delaunay (spade), mit flächenübergreifend geteilten Rand-Ketten → das Netz einer Box ist ein geschlossenes, wasserdichtes 2-Manifold (kein Loch, kein Dreieck über eine Kante). Gekrümmte Flächen (Zylinder/Kugel/Torus) folgen als nächster Schritt. Das Debug-Overlay zeichnet die Punkte an ihrer echten Position (kein Z-Offset/-Versatz); die Sichtbarkeit regelt der x-ray-Layer wie beim PASM-Mesher-Overlay.",
      },
    ],
  },
  {
    version: "0.6.488",
    entries: [
      {
        kind: "feat",
        scope: "issues",
        tickets: ["066230d9"],
        text: "Issues haben jetzt eine kurze Referenz „#N“ statt der langen UUID — angezeigt in der Liste (eigene, sortierbare Spalte) und im Detail-Kopf. Die interne ID bleibt der Speicher-/Routing-Schlüssel; die Nummer ist die menschen- und agentenfreundliche Kurzform (im lokalen Store fortlaufend, in Prod vom Issue-Server pro Projekt vergeben — mit dem issuevault-Agenten abgestimmt). Außerdem kann der Autor den State seines Tickets selbst steuern: schließen, ein geschlossenes wieder öffnen, und neu „Backlog“ = notiert, aber vom Agenten vorerst nicht bearbeiten (parken/entparken). Serverseitig als sichere Teilmenge der State-Machine (nur der Autor bzw. Admins), inkl. Enum-Migration und Backfill der Bestands-Nummern.",
      },
    ],
  },
  {
    version: "0.6.487",
    entries: [
      {
        kind: "fix",
        scope: "custom-sdf",
        tickets: ["c50f3e15"],
        text: "Custom SDF ist jetzt editierbar. Zwei Probleme behoben: das Feature meldete intern die Art „unknown“, weshalb das Detail-Panel „Kein Feature selektiert“ zeigte statt eines Dialogs — die Formel blieb die Default-Kugel. Jetzt öffnet die Auswahl des Custom-SDF-Features einen Formel-Editor: ein mehrzeiliges Monospace-Feld mit Vokabular-Hinweis (Variablen x, y, z · Funktionen sin cos tan sqrt … · Konstanten pi, tau · Potenz ^). Syntaxfehler in der Formel werden als Feature-Fehler angezeigt. So kann man z.B. eine Gyroid-Formel eingeben: sin(x)·cos(y) + sin(y)·cos(z) + sin(z)·cos(x).",
      },
    ],
  },
  {
    version: "0.6.486",
    entries: [
      {
        kind: "fix",
        scope: "cgal",
        tickets: ["335d153"],
        text: "CGAL-Export: scharfe, gekrümmte B-Rep-Kanten (z.B. ein Sichelrand) fransen nicht mehr aus. Zwei Ursachen behoben: die Schutz-Polylinien der Feature-Kanten wurden zu grob vereinfacht (0,1·cell → eine gekrümmte Kante wurde zu wenigen langen Geraden, deren Silhouette zackte) — jetzt 0,02·cell, also kurvenerhaltend; und die neue edge_distance-Kriterium (CGAL 6.0) begrenzt den Sehnenfehler der vermaschten Kante wie facet_distance für die Fläche. Ergebnis: der obere Sichelbogen wird sauber, das Netz bleibt wasserdicht/2-manifold. Sehr dünne Spitzen brauchen weiterhin eine kleinere Zellgröße.",
      },
    ],
  },
  {
    version: "0.6.485",
    entries: [
      {
        kind: "feat",
        scope: "issues",
        tickets: ["a7b63b6"],
        text: "Die Issues-Liste hat jetzt klickbare, sortierbare Spaltenüberschriften (State, Title, Category, Updated). Ein Klick sortiert nach der Spalte, ein weiterer kehrt die Richtung um; die aktive Spalte zeigt einen Pfeil. Text-Spalten starten A→Z, „Updated“ neueste zuerst.",
      },
    ],
  },
  {
    version: "0.6.484",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Fillet-Flächen weben jetzt sauber (c-M2): Die tangentialen Übergangsnähte eines Fillets (wo die Rundung ihre Trägerflächen G1-tangential verlässt) kann der numerische Tracer prinzipbedingt nicht abtasten — sie werden nun symbolisch als exakte Kurven eingesetzt. Für einen Fillet zwischen Zylinder und ebener Fläche (ein exakter Torus-Ring) sind das zwei koaxiale Kreise; das Ergebnis ist ein topologisch geschlossener, manifolder Körper mit einer echten Fillet-Fläche. Die verbleibenden Fälle (gerade Kanten mit Auslauf-Ecken) folgen später.",
      },
    ],
  },
  {
    version: "0.6.483",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Neues Feature „Round All Edges“: rundet in einem Schritt jede Kante global mit einem Radius. Als Feld-Operator (SDF) kann die Rundung den Körper nie in einen ungültigen Zustand bringen, und zwischen ebenen Flächen ist sie ein exakter Kreisbogen. Gerundet werden die Kanten, an denen Features aneinanderstoßen; die Eigenkanten eines einzelnen Primitivs bleiben vorerst außen vor.",
      },
    ],
  },
  {
    version: "0.6.482",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "Fillets sind jetzt exakt kreisförmig: Der Rundungs-Übergang nutzt die zirkuläre smin-Form (echter Kreisbogen vom Radius r zwischen zwei ebenen Flächen) statt der bisherigen kubischen Polynom-Näherung — ein gefilletes Kantenprofil hat damit einen messbaren Radius. Nebenbei die zwei bisher auseinanderlaufenden smin-Familien vereinheitlicht (CPU-Kernel war kubisch, Tape/GPU/CGAL-Mesher quadratisch — beide behaupteten fälschlich „cubic“) und einen fehlenden Nulldivisions-Schutz (blend_radius 0) in der Tape-Familie ergänzt.",
      },
    ],
  },
  {
    version: "0.6.481",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Fillets als eigenständige, referenzierbare Fläche (Fundament): Ein kantenselektiver Fillet erzeugt jetzt einen prozeduralen Blend-Träger mit stabiler, provenienz-basierter Identität (Fillet-Feature + die zwei Wand-Kanten) statt einer namenlosen Ersatzfläche. Das abgeleitete B-Rep folgt der echten Rundung statt der scharfen Phantomkante, und die Blend-Fläche ist damit auswählbar und benennbar — die Grundlage dafür, Fillet-Geometrie später sauber weiterzuverwenden (Projektion in Sketches u.a.). Auswertung konsistent über alle Backends (Dual-Zahlen, Tape/GPU, CGAL-Mesher).",
      },
    ],
  },
  {
    version: "0.6.480",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Tangentialkontakt, Teil 2 — die konstruierten Profil-G1-Fälle: Gestapelte Revolves, deren Profile sich einen Endpunkt mit gleicher Tangente teilen (der Vasen-Stapel: Zylinder-Topf + Bogen-Kuppel), bekommen ihren Übergangs-Kreis jetzt symbolisch aus den exakten 2D-Sketchdaten (Profil-Kettenenden + Endtangenten) — RevolvedProfile-Wände paaren mit RevolvedProfile, Zylinder, Kegel und achsnormalen Ebenen.",
      },
    ],
  },
  {
    version: "0.6.479",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Tangentialkontakt (Kontaktordnung 1) für koaxiale Rotationsflächen: Die Koinzidenz-Registry erkennt Tangentialkreise symbolisch auf Konstruktionsdaten (Kugel↔Zylinder, Kugel↔Kegel, Ebene↔Torus, Zylinder↔Torus, Kugel↔Torus) und der Weave materialisiert den EXAKTEN Kreis statt zu tracen — ein kugelgekappter Zylinder (abgerundeter Pfosten) webt jetzt sauber (vorher 241 Verletzungen auf der Octree-Route), ebenso die abgerundete Kegelspitze. Ein Kontributor-Rollenwechsel-Gate unterscheidet echte Nähte von Innen-Berührungen (einbeschriebene Kugel = keine Kante).",
      },
      {
        kind: "fix",
        scope: "kernel",
        text: "Aufwärts breiter werdende Kegelstümpfe (r_top > r_bottom) verloren auf der Seed-Route ALLE Kanten: primitive_halfspaces erzeugte einen negativen Halbwinkel, der das Steigungs-Vorzeichen der Kegel-SDF drehte — jede Rim-Projektion lief ins Leere. Der Halbwinkel ist jetzt immer positiv (die Öffnungsrichtung trägt der Apex-Offset).",
      },
    ],
  },
  {
    version: "0.6.478",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "Ring-Windung ohne intrinsische Autorität repariert: Punktur-Ringe, die um die Röhre eines Torus (oder durch eine Kugel) winden, haben die Fläche auf BEIDEN Seiten — uv-Shoelace und Flächenseite-Regel können sie nicht orientieren und drehten sie bisher gegen ihre schon korrekten Partner-Faces. Jetzt enthalten sich diese Ringe (und alle Ringe tessellierter Faces) beim intrinsischen Flip und übernehmen per Mehrheits-Parität das Komplement ihrer autoritativen Partner — Box ∪ Torus webt orientierungs-sauber (0 Manifold-Verletzungen) auf Octree- UND Hybrid-Route.",
      },
    ],
  },
  {
    version: "0.6.477",
    entries: [
      {
        kind: "feat",
        scope: "thicken",
        text: "Thicken funktioniert jetzt auch auf Flächen von zusammengesetzten (Boolean-)Features — etwa FieldSkript-Körpern oder verketteten Thicken-Wänden: Der Träger der gepickten Fläche wird per Proben an den Face-Loop-Samples aus dem Boolean-Baum isoliert (nur DIE Fläche wächst, nicht jede Oberfläche des Booleans). Shell-/Offset-Owner ohne eindeutigen geschlossenen Träger melden weiterhin einen klaren Fehler.",
      },
      {
        kind: "fix",
        scope: "extrude",
        text: "Gekrümmte Fläche als Extrude-Profil: früher, präziser Fehler mit Handlungsanweisung (Thicken nutzen) statt des späten generischen Profil-Fehlers. Design-Entscheidung im Kernel-Konzept dokumentiert: Extrude bleibt planar, Normal-Wachstum gekrümmter Flächen ist Thicken.",
      },
    ],
  },
  {
    version: "0.6.476",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Hybrid-Weave (Seed-Weaver Stufe C): Körper, deren Übergangs-Seams analytisch nicht auflösbar sind (z. B. Box ∪ Torus), bekommen jetzt ein VOLLSTÄNDIGES B-Rep statt einer offenen Schale — die parametrisch exakten Kanten der Operanden bleiben erhalten, nur die übersprungenen Seam-Paare werden aus dem Octree-Wireframe nachgefüllt (Endpunkte in die Seed-Ecken verschweißt, Branch-Sets vereinigt). Qualitätsboden: nie schlechter als der bisherige Voll-Octree-Fallback.",
      },
    ],
  },
  {
    version: "0.6.475",
    entries: [
      {
        kind: "fix",
        scope: "shell",
        text: "Shell-Open-Faces erreichen jetzt auch Sweep-Körper: Beide SDF-Evaluatoren (BVH-Hot-Path und Picking-Pfad) gaten das geöffnete Profil-Segment im Querschnitt des Sweeps — Parität mit Extrude/Revolve/Helix, die den Retained-Pfad schon konnten.",
      },
    ],
  },
  {
    version: "0.6.474",
    entries: [
      {
        kind: "feat",
        scope: "thicken",
        text: "Partielle (geschnittene) Zylinder- und Kegelwände lassen sich verdicken: Der Winkel-Spann der Wand wird aus den Rim-Kanten gelesen und die Voll-Rotations-Differenz mit einem extrudierten Sektor-Prisma auf den tatsächlichen Ausschnitt begrenzt — eine Half-Pipe wird zur halben Röhre statt zum Fehler. Außerdem: Up-to-Face akzeptiert jetzt Kegel-Ziele (koaxial, gleicher Halbwinkel — konstanter Wandabstand δ·sin α).",
      },
    ],
  },
  {
    version: "0.6.473",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "Koplanare ON-same-Überlappung webt manifold: Kreuzende Körper mit gleich hohen Deckeln doppel-deckten das Überlappungs-Quadrat (40 Orientierungs-Verletzungen). Neue Reparatur-Stufe im Weave: schlägt die Materialisierung fehl, partitioniert eine ±ε-Ownership-Probe die überlappenden Klassen-Regionen (kleinster enthaltender Member besitzt die Region), vergrabene Signatur-Branches verlieren die Kante, Flächen-innere Nähte fliegen. Nur bei nachgewiesenem Overlap aktiv — anstoßende Stapel (o2j9-Familie) bleiben byte-identisch.",
      },
    ],
  },
  {
    version: "0.6.472",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "DL-82 behoben: Bei feiner Octree-Auflösung erzeugte der Edge-Weave Geisterkanten quer über einen offenen Nutmund (der adaptive Seam-Tracer übersprang die Lücke in einem Schritt, alle Samples lagen auf den Rims). Der Body-Boundary-Filter testet jetzt symmetrisch (vergraben UND in der Luft) und probt bei getracten Chains zusätzlich die Chord-Mittelpunkte gegen das Sagitta-Budget — die Brücke fliegt, echte gekrümmte Kanten bleiben.",
      },
    ],
  },
  {
    version: "0.6.471",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Persistentes Naming N4 — selbstheilende Dokumente: Beim Speichern (inkl. Auto-Save) werden alle gespeicherten Element-Referenzen gegen das aktuelle Modell re-kanonisiert — geheilte Referenzen bekommen frischen Hash + Fingerprint (kein Fingerprint-Verfall über viele Edits), alte hash-only Referenzen werden auf die heilbare Langform aufgerüstet. Unauflösbare Referenzen bleiben unverändert und melden ihren Fehler ehrlich am Feature.",
      },
    ],
  },
  {
    version: "0.6.470",
    entries: [
      {
        kind: "feat",
        scope: "kernel",
        text: "Persistentes Naming N2/N3: Element-Referenzen tragen jetzt einen geometrischen Fingerprint (qFace(<hash> @ …), kanonische Form kommt fertig aus dem Kernel) und HEILEN beim Auflösen — eine gespeicherte Face-/Edge-/Vertex-Referenz überlebt topologische Edits, statt mit „face not found“ zu scheitern. Alle Feature-Pfade (Extrude auf Face, Up-to-Face, Thicken, Shell, Fillet, Mate-Anchor) laufen über eine gemeinsame healing-fähige Auflösung statt sieben Hash-Scan-Kopien.",
      },
    ],
  },
  {
    version: "0.6.469",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "Persistentes Naming N1: Referenz-Auflösung bewertet alle Kandidaten einheitlich (Fingerprint-Ähnlichkeit + Hash-Bonus, provenienz-gebunden auf die Signatur-Gruppe) — ein Index-Kipp bindet nicht mehr still an das falsche Element, gleich gute Kandidaten melden ehrlich „Ambiguous“. Räumliche Index-Vergabe jetzt mit gruppen-relativem ε statt absolutem 1e-9 (nm-Jitter kippt keine Identitäten mehr). Neue Handle-Resolve-Gate-Testfamilie (naming_gate.rs).",
      },
    ],
  },
  {
    version: "0.6.468",
    entries: [
      {
        kind: "fix",
        scope: "mate",
        tickets: ["e94cf61"],
        text: "Mate-Connector-Gizmos (platzierte Connectoren und die Hover-Vorschau) erscheinen jetzt zoomstufen-unabhängig immer gleich groß (~2rem), statt mit der Zoomstufe mitzuwachsen. Die Arme werden pro Frame auf eine konstante Bildschirmgröße skaliert — dieselbe Screen-Space-Logik wie bei den übrigen Gizmos.",
      },
    ],
  },
  {
    version: "0.6.467",
    entries: [
      {
        kind: "fix",
        scope: "debug",
        tickets: ["72a141f"],
        text: "Debug ▸ Part-AABB zeigt jetzt auch die Bounding-Box eines über die Parts-Liste selektierten Körpers. Ein Klick in der Parts-Liste markiert den Körper über den Highlight-Index, nicht über die Feature-Selektion — Overlay und Zahlen-Panel lasen aber nur letztere und blieben daher leer. Beide berücksichtigen nun zusätzlich den hervorgehobenen Part.",
      },
    ],
  },
  {
    version: "0.6.466",
    entries: [
      {
        kind: "fix",
        scope: "mesher",
        tickets: ["0018657"],
        text: "Zahlenfelder, deren Änderung ein teures Neu-Weben auslöst (Mesher-, Mesher-PASM- und Export-Dialog), werten die Eingabe jetzt erst bei Verlassen des Feldes bzw. mit Enter aus. Vorher hing der Mesher schon während der Eingabe: Tippt man „0“ und dann „0,“, wurde der Zwischenwert auf das Minimum geklemmt und ein extrem feines Mesh gewoben, obwohl die Eingabe noch gar nicht fertig war.",
      },
    ],
  },
  {
    version: "0.6.465",
    entries: [
      {
        kind: "feat",
        scope: "mesher",
        tickets: ["nwmi"],
        text: "Mesher PASM: Krümmungs-Anisotropie (Paper §3.2, opt-in Checkbox im PASM-Dialog). Die Gauß-Paarkraft wird mit der Flächenmetrik M=QᵀQ aus dem SDF-Formoperator verzerrt (Paper Gl. 21, Q_ij statt M_ij — der ASPH-Fehler wird vermieden): Partikel packen dichter, wo die Fläche krümmt, und gröber, wo sie flach ist. Am langen Zylinder messbar weniger Dreiecke bei gleicher Sag-Toleranz, gestreckte Dreiecke in der flachen Richtung, weiterhin wasserdicht/2-manifold. Bewusst opt-in (nicht im Export-Default), da der Formoperator pro Partikel die Vernetzung deutlich verlangsamt.",
      },
    ],
  },
  {
    version: "0.6.464",
    entries: [
      {
        kind: "feat",
        scope: "export",
        tickets: ["nwmi"],
        text: "Export-Stufe „CAD (kantenexakt, PASM+B-Rep)“ — die Hybrid-Pipeline (Skelett → PASM-Relaxation → per-Face-CDT → DC-Fallback) ist jetzt ein wählbares Meshing-Verfahren im Export-Dialog neben MDC/MC/Surface-Nets. Die Zellgröße wird dabei als Ziel-Kantenlänge h gelesen. Ein Geschlossenheits-Gate fängt Körper ab, die die PASM-Kette nicht dicht bekommt (z.B. unvollständig gewebte Gyroide) und fällt für diesen Körper sauber auf das adaptive Verfahren zurück, sodass der Export nie ein Loch ausliefert.",
      },
    ],
  },
  {
    version: "0.6.463",
    entries: [
      {
        kind: "feat",
        scope: "mesher",
        tickets: ["nwmi"],
        text: "Mesher PASM: Fallback für Flächen ohne Chart (Torus-/Gyroid-Träger, Custom-SDF) — die Region wird als Signed-DC-Octree-Patch vermascht, per Reißverschluss strukturell wasserdicht an die exakten B-Rep-Rim-Ketten genäht (geteilte Indizes, Loch-Füllung, Komponenten-Gate) und im Dialog orange dargestellt. Neuer Schritt-Selektor (Seeds → Skelett → Relax → CDT → Fallback) zeigt jede Pipeline-Stufe einzeln in der Szene; Kernel-Test: Torus ∩ Quader end-to-end geschlossen, 2-manifold, Euler 0.",
      },
    ],
  },
  {
    version: "0.6.462",
    entries: [
      {
        kind: "fix",
        scope: "export",
        tickets: ["cdb25e1e"],
        text: "Export-Preview: Wireframe-Checkbox zeigt jetzt auch in den SDF-Rendermodi ein Drahtgitter (WebGPU ignoriert material.wireframe — das Gitter ist jetzt echte Linien-Geometrie), und der Deckkraft-Regler wirkt stufenlos statt als An/Aus (konstante Alpha-Blend-Pipeline statt Pipeline-Rebuild pro Reglerschritt).",
      },
    ],
  },
  {
    version: "0.6.461",
    entries: [
      {
        kind: "feat",
        scope: "mesher",
        tickets: ["nwmi"],
        text: "Debug ▸ „Mesher PASM…“ — eigener Dialog für die Hybrid-PASM-Pipeline (B-Rep-Skelett → Gauß-Relaxation → per-Face-CDT) mit genau einem Auflösungsparameter: der Ziel-Kantenlänge h. Seed-Octree-Tiefe, Kernel-Radius (5σ = h), Ketten-/Naht-Abstand und CDT-Clearance leitet der Kernel daraus ab (neues targetEdgeLength in mesherPoints). Der generische Mesher-Dialog bleibt als Baukasten erhalten; beide schließen sich gegenseitig aus.",
      },
    ],
  },
  {
    version: "0.6.460",
    entries: [
      {
        kind: "fix",
        scope: "dev",
        tickets: ["dc02f591"],
        text: "„Auto-reload on code change: Off“ schottet die App jetzt komplett von Code-Änderungen ab: Bisher blieben Hot-Module-Updates aktiv und setzten beim Editieren von Quellcode den UI-Zustand zurück (ohne Reload). Der Client meldet die Einstellung an den Dev-Server, der daraufhin sämtliche Hot-Payloads (Updates, Prune, Full-Reload) für diesen Tab verwirft — F5 lädt weiterhin den neuesten Stand, und das Umschalten wirkt sofort ohne Neustart.",
      },
    ],
  },
  {
    version: "0.6.459",
    entries: [
      {
        kind: "feat",
        scope: "projects",
        tickets: ["14661fc4"],
        text: "Lokale Projekte erscheinen jetzt in der Projekt-Übersicht („On this device“) und lassen sich von dort wieder öffnen: „Open local project…“ nutzt die File-System-Access-API, merkt sich das FileHandle in IndexedDB, und der Autosave schreibt die .fdraft-Datei fortan direkt auf der Platte mit (gleiche Bytes wie „Download project“). „Forget“ entfernt einen Eintrag aus der Liste (die Datei bleibt). Auf Browsern ohne File-System-Access-API bleibt der klassische Datei-Dialog ohne Merken/Disk-Autosave.",
      },
    ],
  },
  {
    version: "0.6.458",
    entries: [
      {
        kind: "feat",
        scope: "app",
        tickets: ["3032c29b"],
        text: "URL-Routing: Issues und Projekte haben jetzt URLs (#/issues, #/issues/<id>, #/projects, #/p/<projekt-id>, #/edit) — Browser-Zurück funktioniert (z. B. vom Issue-Detail zur Liste), und die URLs sind teil-/lesezeichenbar: ein direkt geladener #/p/<id>-Link öffnet das Server-Projekt, #/issues/<id> das Ticket.",
      },
    ],
  },
  {
    version: "0.6.457",
    entries: [
      {
        kind: "feat",
        scope: "ui",
        tickets: ["6c09c11f"],
        text: "Burger-Menü unterscheidet lokal vs. Cloud: „Save“ heißt jetzt „Download project“ (lokale .fdraft-Datei, Ctrl+S), „Open…“ heißt „Open local project…“, und ein neuer Punkt „Save to cloud“ macht aus dem offenen lokalen Projekt ein Cloud-Projekt (legt es auf dem Server an und bindet es — ab dann greift der Autosave). Bei bereits gebundenen Projekten ist der Punkt mit Hinweis deaktiviert.",
      },
    ],
  },
  {
    version: "0.6.456",
    entries: [
      {
        kind: "feat",
        scope: "projects",
        tickets: ["421c3610"],
        text: "Neuer „Import…“-Button neben „New project“ auf der Projekt-Seite: importiert ein lokales .fdraft als neues Server-Projekt (benannt nach der Datei) und öffnet es direkt gebunden.",
      },
    ],
  },
  {
    version: "0.6.455",
    entries: [
      {
        kind: "feat",
        scope: "tickets",
        tickets: ["d22a909e"],
        text: "Tickets lassen sich wieder öffnen: Der Autor eines resolved-Tickets hat neben „Close issue“ jetzt einen „Reopen“-Button (setzt zurück auf in_progress — „der Fix reicht nicht“). Andere Zustandswechsel bleiben Admin/PO-Sache.",
      },
    ],
  },
  {
    version: "0.6.454",
    entries: [
      {
        kind: "fix",
        scope: "ui",
        tickets: ["b5a60122"],
        text: "„Report Issue…“ ist zurück im Burger-Menü (mit sichtbarem F2-Shortcut) und aus dem User-Menü entfernt — das User-Menü ist wieder nur Identität + Log out.",
      },
    ],
  },
  {
    version: "0.6.453",
    entries: [
      {
        kind: "fix",
        scope: "tickets",
        text: "„Report an issue“ scheiterte mit einem 500er, wenn in den ersten 80 Zeichen der Beschreibung ein Umlaut auf der Kürzungsgrenze lag: Der abgeleitete Titel wurde byteweise statt zeichenweise gekürzt, das halbe Multibyte-Zeichen war ungültiges UTF-8 und die Datenbank lehnte den Eintrag ab. Der Titel wird jetzt runen-sicher gekürzt.",
      },
    ],
  },
  {
    version: "0.6.452",
    entries: [
      {
        kind: "fix",
        scope: "viewport",
        tickets: ["pzvt"],
        text: "Kanten-Hover trifft wieder, auch wenn der sichtbare Frame während langer Shader-Compiles eingefroren ist („Pick what you see“): Die SDF-Renderer publizieren die Kamera jedes tatsächlich gezeichneten Frames, und Edge-/Face-/Vertex-Picking projiziert gegen diesen Snapshot statt gegen die Live-Kamera. Bisher desyncte jede Kamera-Änderung im Compile-Fenster (View-Restore, kleine Orbits) Interaktion und Anzeige — das 8px-Kanten-Band verfehlte zuerst, Flächen schienen weiter zu funktionieren.",
      },
    ],
  },
  {
    version: "0.6.451",
    entries: [
      {
        kind: "feat",
        scope: "tickets",
        text: "Links in Issue-Beschreibungen und Kommentaren sind jetzt klickbar (öffnen in neuem Tab) — z. B. die Commit-Links in Dev-Kommentaren.",
      },
    ],
  },
  {
    version: "0.6.450",
    entries: [
      {
        kind: "fix",
        scope: "ui",
        tickets: ["3d0f20de"],
        text: "„Open…“ funktioniert jetzt auch von der Projekt-Übersicht aus: Das gewählte lokale .fdraft wird geladen und die App wechselt in die Modellier-Ansicht. Bisher lud die Datei unsichtbar in den Hintergrund-Workspace („es passiert nichts“); außerdem wird vorher die Server-Bindung gelöst, damit der Autosave das gebundene Server-Projekt nicht mit dem lokalen File überschreibt.",
      },
      {
        kind: "fix",
        scope: "tickets",
        tickets: ["ddd6d897"],
        text: "„Attach screenshot“ geht jetzt auch auf der Projekt-Übersicht. Der Capture hing bisher endlos an zwei Stellen: an den (noch leeren) 404-Projekt-Thumbnails und am Klonen der unsichtbar mitgemounteten 3D-Canvas. Kaputte Bilder und hidden-Bereiche werden jetzt aus dem Capture gefiltert, dazu ein 8-Sekunden-Sicherheitslimit.",
      },
      {
        kind: "feat",
        scope: "tickets",
        tickets: ["582dfe62"],
        text: "Angehängte Bilder in der Issue-Detailansicht zeigen jetzt ein Preview-Thumbnail (wie der Fenster-Screenshot) statt eines reinen Datei-Chips.",
      },
    ],
  },
  {
    version: "0.6.449",
    entries: [
      {
        kind: "fix",
        scope: "dev",
        tickets: ["a020ac1e"],
        text: "„Auto-reload on code change: Off“ wirkt wieder. Vite 8 führt HMR-Listener über Promise.allSettled aus, wodurch der bisherige Abbrech-Trick (Exception im vite:beforeFullReload-Handler) still wirkungslos wurde — die Seite lud trotz Off-Schalter neu. Der Dev-Server schickt jetzt nie mehr ein rohes full-reload; der Client entscheidet selbst anhand der Einstellung.",
      },
      {
        kind: "fix",
        scope: "ui",
        tickets: ["376bf453"],
        text: "Burger-Menü: Dokument-Aktionen (New, Open, Save, Import, Export, Share, Undo, Redo, Debug) sind außerhalb der Projekt-Ansicht deaktiviert — auf der Projekt-Seite operierten sie bisher unsichtbar auf dem Hintergrund-Workspace.",
      },
      {
        kind: "fix",
        scope: "ui",
        tickets: ["e752b6f8"],
        text: "„Report Issue…“ und „Release Notes…“ sind aus dem Burger-Menü entfernt. Beides bleibt erreichbar: Issues über F2, Release Notes über Klick aufs Versions-Badge.",
      },
    ],
  },
  {
    version: "0.6.448",
    entries: [
      {
        kind: "fix",
        scope: "export",
        tickets: ["63989a9e"],
        text: "Server-Projekt-Bindung überlebt jetzt einen Seiten-Reload: Bisher stellte das Autosave zwar das Dokument wieder her, die Sync-Bindung ging aber verloren — der CGAL-Server-Export meldete dann fälschlich „save to server first“, obwohl das Projekt auf dem Server lag. Die Bindung wird jetzt persistiert und beim Start gegen den Server verifiziert wiederhergestellt (gelöschte/fremde Projekte binden nicht).",
      },
      {
        kind: "fix",
        scope: "projects",
        tickets: ["63989a9e"],
        text: "Ein Server-Projekt mit defektem Dokument-Blob öffnete sich bisher STILL falsch: Die App wechselte in die Modellier-Ansicht, zeigte dort aber das vorherige lokale Dokument ungebunden an — mit demselben „save to server first“-Symptom. Jetzt bleibt die App auf der Projekt-Seite und zeigt eine Fehlermeldung.",
      },
    ],
  },
  {
    version: "0.6.447",
    entries: [
      {
        kind: "perf",
        scope: "export",
        text: "CGAL-Server-Meshing: zell-lokale Opcode-Tape-Spezialisierung (C++-Gegenstück zu cell_csg.rs) — pro Octree-Zelle werden Boolean-Zweige, die das Feld dort beweisbar nicht beeinflussen, per 1-Lipschitz-Ball-Bound geprunt und die verkürzte Tape an Kindzellen vererbt; das Mesh_3-Oracle evaluiert nur noch die Zell-Tape. 512-Teile-Baugruppe: 2,2× Wall / 4,0× CPU (Tape 6656 → Ø 30 Records); bit-exakt verifiziert (check_spec-Harness, 7 Modelle × 20k Zufallspunkte, Abweichung 0). FD_NO_SPEC=1 schaltet ab.",
      },
    ],
  },
  {
    version: "0.6.446",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "Custom-SDF-Körper (Math-Expressions) sind jetzt auch über das Server-Meshing exportierbar: OP_PRIM_CUSTOM (0x0E, Expression-Stack-Machine) ist in den C++-CGAL-Interpreter portiert, das Ausschluss-Gate in fd-prepare entfällt. Golden-Szene (sin-displaced Sphere, 26 Szenen gesamt) und End-to-End-Verifikation (wasserdicht/2-manifold) grün.",
      },
    ],
  },
  {
    version: "0.6.445",
    entries: [
      {
        kind: "feat",
        text: "Neues Feature „Custom SDF“ (ƒ in der Toolbar): eigene implizite Körper aus einer Math-Formel über x, y, z — z. B. TPMS-Gitter oder algebraische Flächen. Die Formel (sin, cos, sqrt, abs, min, max, exp, ln, atan2, pow, ^, pi/tau) wird einmal in eine Opcode-Tape kompiliert und identisch auf CPU (inkl. exakter Gradienten) und GPU ausgewertet; Lipschitz-Faktor normalisiert das Feld, eine lokale Box begrenzt den Körper. Custom-Bodies sind SDF-only (kein B-Rep), wie das Gyroid",
        scope: "kernel",
      },
      {
        kind: "perf",
        text: "Zell-lokale CSG-Baum-Spezialisierung im Kernel: B-Rep-Octree und adaptiver Narrow-Band-Mesher prunen pro Zelle Boolean-Zweige, die das Feld dort beweisbar nicht beeinflussen (fidget-artige Tape-Verkürzung auf dem eigenen Baum), und vererben den verkürzten Baum an die Kindzellen — Octree-Aufbau auf zusammengesetzten Körpern mehrfach schneller (30-Teile-Testbaugruppe: 3,9×), byte-identisches Ergebnis. Zusätzlich eps-bewusstes AABB-Pruning in der Contributor-Evaluation",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.444",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "CGAL-Server-Meshing versteht jetzt alle Kernel-Opcodes: HALFSPACE (geöffnete Shells), Helix und Sweep sind in den C++-Interpreter portiert, ein Masked-Blend-Format-Drift ist gefixt (count-präfixierte Polyline statt festem 2-Punkt-Kapsel-Layout). Der Golden-Test deckt alles mit 9 neuen Szenen ab (25 gesamt, 12.800 Checks). Dazu Douglas-Peucker-Dezimierung der Kanten-Schutz-Polylines im Mesher — eine Helix lieferte 102.000 Tracer-Punkte und würgte CGALs Feature-Protection ab. Helix+Vase end-to-end verifiziert: wasserdicht, 2-manifold, kantentreu (3,4 Mio. Dreiecke).",
      },
    ],
  },
  {
    version: "0.6.443",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "CGAL-Meshing Meilenstein M6 (3MF): Die Server-Methode „CGAL Mesh_3 (server)“ exportiert jetzt auch 3MF — das Server-STL wird clientseitig durch den Kernel-3MF-Encoder konvertiert (neues mesh3mfFromBuffers), das Format-Select ist für die Server-Route freigeschaltet. Damit sind alle Meilensteine des serverseitigen CGAL-Meshings (M1–M6) abgeschlossen.",
      },
    ],
  },
  {
    version: "0.6.442",
    entries: [
      {
        kind: "feat",
        scope: "deploy",
        text: "CGAL-Meshing Meilenstein M5 (Prod-Deployment): mesher-Service in docker-compose.prod.yml (internes Netz, horizontal skalierbar via --scale), drittes GHCR-Image im Release-Workflow (Golden-Test läuft im Image-Build), Env-Tuning (MESH_WORKERS/CONCURRENCY/THREADS/TIMEOUT) in .env.prod.example, DEPLOY.md + ARCHITECTURE.md nachgeführt. Ansible-Deploy zieht das Image automatisch mit.",
      },
    ],
  },
  {
    version: "0.6.441",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "CGAL-Meshing Meilenstein M4 (Frontend): neue Export-Methode „CGAL Mesh_3 (server)“ im Export-Dialog — speichert das Projekt, startet den Server-Job, pollt den Status (Warteschlange/Server mesht/Download, mit Cancel), zeigt das STL als Preview in der Szene und lädt die unveränderten Server-Bytes herunter. Nur mit server-gespeichertem Projekt und STL-Format wählbar (sonst ausgegraut).",
      },
    ],
  },
  {
    version: "0.6.440",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "CGAL-Meshing Meilenstein M3 (Backend-Job-Queue): persistente mesh_jobs-Tabelle (überlebt Server-Restart, Boot-Recovery running→queued), Dispatcher-Worker-Pool mit atomarem Claim (replikasicher), Routen POST /api/projects/{id}/mesh → 202 jobId, Polling-Route, STL-Artefakt-Download (owner-gated, Content-addressed Blob), Abbruch-Route; max. 1 aktiver Job pro User. Integrationstest + Live-Roundtrip gegen den echten Mesher-Container verifiziert.",
      },
    ],
  },
  {
    version: "0.6.439",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "CGAL-Meshing Meilenstein M2 (Service-Container): Docker-Image (Debian-CGAL mit GMP/MPFR, TBB-Multithreading mit FD_MESH_THREADS-Deckel, Golden-Test als Pflicht-Build-Schritt) + Go-HTTP-Wrapper (POST /mesh → STL, Concurrency-Limit, Job-Timeout, Größenlimit) + Compose-Service „mesher“ im internen Netz.",
      },
    ],
  },
  {
    version: "0.6.438",
    entries: [
      {
        kind: "feat",
        scope: "export",
        text: "Serverseitiges CGAL-Meshing, Meilenstein M1 (Kern-Pipeline): fd-prepare (natives Kernel-Binary: Dokument → SDF-Opcodes + B-Rep-Kanten-Polylines) + fd-cgal-mesh (natives CGAL Mesh_3 mit Kanten-Schutzkugeln, manifold-Kriterium, Außen-Orientierung, binäres STL). Box/Zylinder/Revolve-Rohr verifiziert wasserdicht/2-manifold mit exakter Kantentreue; Golden-Test Rust↔C++ fand und fixte zwei Interpreter-Drifts (Cone wall_scale, Shell inward). Noch ohne UI — Backend-Route und Export-Dialog-Option folgen.",
      },
    ],
  },
  {
    version: "0.6.437",
    entries: [
      {
        kind: "feat",
        scope: "debug",
        tickets: ["nwmi"],
        text: "Hybrid-PASM P4c: Polkappen-Fächer — Faces mit einer windenden Loop um einen Pol/Apex (Kugelkappe, Kegelspitze, Revolve-Achsberührung) werden jetzt vernetzt: ein Pol-Ring aus Hilfspunkten knapp vor dem Pol, das Stück Loop↔Ring über den Band-Pfad, der Ring per Dreiecks-Fächer zum Pol geschlossen — jedes Fächer-Dreieck berührt den Pol genau einmal, nichts kollabiert. Ring-Auflösung aus der Welt-Bogenlänge (am Pol schrumpft der Umfang). Kugelkappe ∩ Box end-to-end geschlossen/2-Manifold (Euler=2).",
      },
    ],
  },
  {
    version: "0.6.436",
    entries: [
      {
        kind: "feat",
        scope: "debug",
        tickets: ["nwmi"],
        text: "Hybrid-PASM P4b: Revolve-Träger im CDT-Mesher — die v-Achse des Charts ist die Bogenlänge der Nächster-Punkt-Projektion auf die offset-korrigierte Profilkette (ssi-Profile-Pieces). Rohr-Meilenstein grün: Rechteck-Revolve ∩ Box liefert end-to-end ein wasserdichtes Torus-Netz (Euler=0) — Außen- und Innenwand liegen auf demselben Branch und trennen sich rein über das Chart, die Deckel sind Annuli mit Loch-Loop. Faltungs-Guard: springt v entlang einer Loop um mehr als ¼ der Kettenlänge, bleibt die Face unvermascht.",
      },
    ],
  },
  {
    version: "0.6.435",
    entries: [
      {
        kind: "feat",
        scope: "debug",
        tickets: ["nwmi"],
        text: "Hybrid-PASM P4a: CDT auf abrollbaren Trägern — Zylinder/Kegel/Kugel-Bänder werden ins (θ·r, v)-Chart abgewickelt und periodisch vernetzt; die Schnitt-Naht teilt ihre Steiner-Punkte zwischen beiden Kopien und ist damit strukturell verschweißt. Voller Zylinder jetzt end-to-end geschlossen/2-Manifold (Band + Deckel, Euler=2). Dreiecks-Klassifikation jetzt kombinatorisch (Flood-Fill mit Constraint-Parität) statt Zentroid-Test — robust gegen Naht-Sliver; randnahe Partikel (<0,25·h) werden vor dem Einfügen verworfen, damit kein einseitiger Constraint-Split die Naht öffnet.",
      },
    ],
  },
  {
    version: "0.6.434",
    entries: [
      {
        kind: "feat",
        text: "Hybrid-PASM-Meshing Phasen 2+3: „PASM kernel (Gaussian)“ — der Paar-Kernel des Papers (σ = Interaktionsradius/5, 5σ-Trunkierung, Konvergenz-Abbruch) ersetzt wahlweise den 1/d²-Monopol; „CDT faces (planar)“ — planare Faces werden per Constrained Delaunay (spade) in der Face-Ebene vernetzt, mit den B-Rep-Skelett-Loops als Constraints und den freien Partikeln als Innenpunkten. Erster End-to-End-Beweis: die Box liefert ein GESCHLOSSENES 2-Manifold-Netz (jede Kante exakt 2×, Euler V−E+F=2) — kantenexakt und wasserdicht per Konstruktion",
        scope: "debug",
        tickets: ["nwmi"],
      },
    ],
  },
  {
    version: "0.6.433",
    entries: [
      {
        kind: "feat",
        text: "Hybrid-PASM-Meshing Phase 1 (Konzept-SSOT docs/HYBRID-PASM-MESHING-PLAN.md): Debug ▸ Mesher kann die gewebten B-Rep-Kanten als FIXES Partikel-Skelett einspannen — Ecken exakt gepinnt (rot), Kantenketten bogenlängen-gesampelt am Interaktionsradius (orange); sie stoßen die freien Flächen-Partikel ab, bewegen sich aber nie. Scharfe Features werden Randbedingung statt Detektionsaufgabe — die konstruktive Basis für kantenexaktes, wasserdichtes Meshing",
        scope: "debug",
        tickets: ["nwmi"],
      },
    ],
  },
  {
    version: "0.6.432",
    entries: [
      {
        kind: "fix",
        text: "Schmale Flächen waren nicht hoverbar (nur einzelne Kanten leuchteten): der Kanten-Pick-Radius war in NDC definiert und wuchs mit der Fenstergröße auf ~15px — breit genug, um eine schmale Seitenfläche von beiden Silhouetten her komplett abzudecken, sodass per Edge-vor-Face-Priorität nie die Fläche gewann. Kanten- (8px) und Vertex-Pick-Radius (11px) sind jetzt pixel-basiert und fenstergrößen-stabil",
        scope: "viewport",
        tickets: ["7cv2"],
      },
    ],
  },
  {
    version: "0.6.431",
    entries: [
      {
        kind: "feat",
        text: "B-Rep: Vertex-Witnesses — Ecken (Tripelpunkte) werden VOR dem Kanten-Tracing analytisch gelöst und den Märschen als Snap-Ziele mitgegeben; ein Marsch endet exakt an der Ecke statt eine weggetrimmte Lücke zu überspringen (fundamentale Lösung des Gap-Leap-Problems). Schutzgitter: Aktivitäts-Gate über alle drei Flächenpaare, Spatprodukt-Rang-Gate gegen Phantom-Konkurrenzen entlang µm-gedrifteter Koplanar-Wände, Mikro-Stub-Filter; Branch-Union wie im Cluster-Pfad hält die Handle-Identität zur Octree-Route",
        scope: "kernel",
      },
      {
        kind: "fix",
        text: "Zoom zum Mauszeiger (Ortho): der Pin-Offset wurde relativ zum Orbit-Pivot statt zur Kamera-Achse gerechnet — seit dem Onshape-Pivot-Raycast liegt der Pivot oft abseits der Blickachse, dann zoomte das Mausrad am Cursor vorbei (aufs Bildschirmzentrum). Jetzt bleibt der Punkt unterm Cursor exakt stehen",
        scope: "camera",
        tickets: ["zc9b"],
      },
    ],
  },
  {
    version: "0.6.430",
    entries: [
      {
        kind: "fix",
        text: "B-Rep: fehlende vertikale Kanten, wo eine große Zylinderwand eine kleine Box-Seitenfläche schneidet (Gyroid-Sliver). Drei Wurzelursachen behoben: (1) der SSI-Scan verwarf Schnitt-Zellen mit Eckpunkt knapp außerhalb der Overlap-Box — die Ecken-Maske wird jetzt um eine Zell-Diagonale erweitert (Seeds bleiben strikt in der Box); (2) die Co-Component-Dedup fraß parallel laufende Zwillings-Kanten — Radius von 0.1·diag auf 10× Sagitta-Toleranz verkleinert, Trim-Termini zählen zur Dedup-Polyline; (3) ein langer adaptiver Trace-Schritt konnte eine weggetrimmte Lücke komplett überspringen und zwei getrennte Kantenbögen überbrücken — neuer Mittelpunkts-Gap-Guard im March plus halbierte Seed-Routen-Schrittkappe",
        scope: "kernel",
        tickets: ["7sp1"],
      },
    ],
  },
  {
    version: "0.6.429",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: „Feature snap (edges/corners)“ — Kanten/Ecken werden per Normalen-Proben erkannt, Punkte exakt auf den Schnitt der Tangentialebenen gezogen und dort verankert (Kantenpunkte gleiten 1D entlang der Kante, Ecken sind fixiert; beide stoßen weiter ab). Löst das systematische Abrunden harter Kanten durch isotrope Repulsion (0D/1D-Features haben Maß null); einstellbarer Feature-Winkel",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.428",
    entries: [
      {
        kind: "feat",
        text: "Gyroid-Körper haben jetzt B-Rep-Kanten: die Boolean-Rim-Kurven (wo Box/Zylinder/Kugel-Flächen die Gyroid-Shell schneiden) werden über den octree-freien Seed-Pfad exakt getraced (SSI-Seeds + Dual-Field-Tracing) und als partielles Wireframe gewebt. Der Octree bleibt für Space-Filling-Körper weiterhin ausgeschlossen (kein DL-71-Stall); Helix/Sweep-Körper bleiben unverändert kantenlos",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.427",
    entries: [
      {
        kind: "feat",
        text: "Kernel-Spike: Der SSI-Kurven-Seeder unterstützt jetzt Gyroide als Implicit-Operand (Ebene/Zylinder ∩ Gyroid) — Machbarkeitsnachweis für exakte Boolean-Kanten an TPMS-Körpern (Box∩Gyroid: 66 Rim-Kurven, ~9k Seeds, wenige ms). Noch nicht im B-Rep-Weave aktiv; Gyroid-Bodies bleiben vorerst ohne Kanten (DL-71)",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.426",
    entries: [
      {
        kind: "fix",
        text: "Debug ▸ Mesher: „Damping“ ist jetzt ein direktes Zahlenfeld statt eines Sliders",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.425",
    entries: [
      {
        kind: "fix",
        text: "Debug ▸ Mesher: Punkte verklumpten trotz starker 1/d²-Repulsion in einer Ecke — Ursache war eine numerische Instabilität: der explizite Schritt konnte bei hoher „Strength“ einen Punkt komplett über seine Nachbarn hinweg und von der Oberfläche schleudern (wo „Only on surface“ ihn verwarf), während der symmetrische Cluster-Kern (Netto-Kraft ≈ 0) liegen blieb. Der Repulsions-Schritt wird jetzt auf die halbe Distanz zum nächsten Nachbarn begrenzt → stabil für JEDE Strength, Klumpen dehnen sich monoton zu gleichmäßigem Abstand",
        scope: "debug",
        tickets: ["l45r"],
      },
    ],
  },
  {
    version: "0.6.424",
    entries: [
      {
        kind: "fix",
        text: "Debug ▸ Mesher: Repulsions-Kraftgesetz ist jetzt durchgängig invers-quadratisch (∝ 1/d², wie gleich geladene Monopole) — nähere Punkte stoßen sich viel stärker ab, dadurch bleiben Cluster nicht bestehen und die Verteilung bleibt lokal und gleichmäßig statt global in die Ecken zu driften. Die „Weighting“-Auswahl (Linear/Inverse Square) entfällt, 1/d² ist das einzige Gesetz",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.423",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: UI an das Ein-Schleifen-Verfahren angepasst — die zwei alten Iterations-Felder durch ein einziges „Iterations“ ersetzt; neu: Repulsion-„Damping“ (Abkling-Faktor der Repulsion pro Iteration) und „Damping curve“ (Linear/Exponential) für das Annealing",
        scope: "debug",
        tickets: ["yanv"],
      },
    ],
  },
  {
    version: "0.6.422",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: Punktverteilung jetzt eine EINZIGE interleavte Schleife (Repulsion → Fit → Clamp pro Iteration) mit von stark nach null abklingender Repulsion, plus abschließendem Fit-to-Surface — die Punkte spreizen sich früh und liegen am Ende wirklich auf der Oberfläche (im Rahmen des Clamps)",
        scope: "debug",
        tickets: ["yanv"],
      },
    ],
  },
  {
    version: "0.6.421",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: Punktverteilung läuft jetzt als 2-Phasen-Iteration aus atomaren Schritten — Phase 1 pro Schritt Fit-on-Surface → Clamp-to-Cell (bis Target-ε oder Max-Iterationen), Phase 2 pro Schritt Repulsion → Fit → Clamp (für Repulsion-Iterationen); jeder Schritt nur, wenn die zugehörige Checkbox gesetzt ist",
        scope: "debug",
        tickets: ["yanv"],
      },
    ],
  },
  {
    version: "0.6.420",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher „Mesh points“ verbindet jetzt die angezeigten (gefilterten) Punkte entlang der Octree-Kantenbeziehungen zu Dreiecken — kein Dual Contouring, kein Vorzeichenwechsel-Test, Löcher/falsche Facetten sind bewusst erlaubt; das Ergebnis wird nur als Wireframe gezeichnet",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.419",
    entries: [
      {
        kind: "fix",
        text: "Debug ▸ Mesher: die Filter „Neighbour sign change only“ und „Only on surface“ blenden die Normalen/Punkte jetzt sichtbar aus — der Kernel liefert pro Punkt ein shown-Flag, das alle Prozess-Filter berücksichtigt (vorher wirkten sie nur intern auf Fit/Repulsion/Mesh)",
        scope: "debug",
        tickets: ["jt76", "gjuu"],
      },
      {
        kind: "fix",
        text: "Von Disk geöffnetes Projekt überlebt jetzt einen Reload: ein „Projekt öffnen“ persistiert sofort in localStorage und holt einen Deep-Link-Repro-Tab (?project=) aus dem Repro-Modus (echter Autosave, URL-Parameter entfernt), statt beim Reload leer zu sein",
        scope: "project",
        tickets: ["4z37"],
      },
    ],
  },
  {
    version: "0.6.418",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: Filter „Only on surface“ — Punkte, die die Oberfläche nach der Newton-Projektion nicht erreichen (in ihrer Zelle geclampt), fallen komplett aus dem Set: nicht gezeichnet, keine Repulsion, kein Mesh-Vertex",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.417",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: „Mesh points (dual contour)“ — adaptives Dual Contouring über den gefilterten/projizierten/abgestoßenen Punkten (ein Vertex pro Octree-Blattzelle, Quads an vorzeichenwechselnden Minimal-Kanten, crack-frei per rekursivem cell/face/edge-Walk), optional mit 2:1-balanciertem Octree; Dreieckszahl im Dialog",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.416",
    entries: [
      {
        kind: "fix",
        text: "Export-Vorschau: SDF ↔ Mesh-Crossfade blendet wieder graduell — nach dem jüngsten Tiefen-Fix verlor das exakt auf der Fläche liegende Preview-Mesh den Depth-Test gegen den SDF-Körper; es zieht sich jetzt per polygonOffset nach vorn",
        scope: "export",
        tickets: ["rfm6"],
      },
    ],
  },
  {
    version: "0.6.415",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Mesher: iterative Punkt-Optimierer — „Fit on Surface“ (gedämpfte Newton-Projektion auf die SDF-Nullfläche) und „Repulsion inside Surface“ (tangentiale Relaxation mit Re-Pinning), mit Zell-Clamping (constrainToCell), Vorzeichenwechsel-Nachbar-Filter und Gewichtungsfunktion Linear/Inverse Square — komplett im Kernel (31k Punkte fitten in ~70 ms)",
        scope: "debug",
        tickets: ["jq2t"],
      },
    ],
  },
  {
    version: "0.6.414",
    entries: [
      {
        kind: "feat",
        text: "Debug-Dialoge (SDF-Plane, Mesher, Part-AABB) persistieren im Projekt — ein gespeichertes Projekt oder Ticket öffnet mit exakt dem Debug-Setup (offene Dialoge + Parameter), mit dem es erfasst wurde",
        scope: "debug",
      },
      {
        kind: "feat",
        text: "Projekt-Autosave per Dirty-Tracking: auch reine Kamera-Bewegungen und Debug-Dialog-Änderungen speichern jetzt (debounced); Debounce als Einstellung „Autosave-Debounce (s)“ (Default 5 s), beim Schließen des Tabs wird sofort geschrieben",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.413",
    entries: [
      {
        kind: "fix",
        text: "SDF-Renderer: blickwinkelabhängige Parallaxe zwischen Solid und SDF-Plane behoben — die Szenen-Tiefe wurde 0,1 % hinter die Oberfläche geschoben (Kanten-Bias), was an flach geschnittenen Ebenen zur Sichel verstärkte; B-Rep-Kanten ziehen sich jetzt selbst per polygonOffset nach vorn",
        scope: "render",
        tickets: ["iw6f"],
      },
    ],
  },
  {
    version: "0.6.412",
    entries: [
      {
        kind: "fix",
        text: "Extended Marching Cubes: Feature-Zellen zelteten jedes Patch-Dreieck über alle drei Kanten auf den Feature-Vertex — jetzt nur noch über den Patch-Rand (Gyroid-Kugel: 12.650 → 24 non-manifold-Kanten, Abweichung 0,43 → 0,34 mm)",
        scope: "export",
        tickets: ["4m67"],
      },
      {
        kind: "fix",
        text: "Loch-Sicherheitsnetz auch für explizit gewählte Dense-Mesher: Manifold DC lieferte auf der Gyroid-Kugel 1.471 offene Kanten und 7,6 mm Ausreißer — bei Löchern ersetzt jetzt ein Marching-Tets-Netz das Ergebnis (im route-Label ausgewiesen)",
        scope: "export",
        tickets: ["4m67"],
      },
      {
        kind: "fix",
        text: "Export-Verfahren „Marching cubes (robust)“ heißt jetzt ehrlich „Marching Tetrahedra (robust)“ — Titel und Beschreibung widersprachen sich",
        scope: "export",
        tickets: ["agco"],
      },
    ],
  },
  {
    version: "0.6.411",
    entries: [
      {
        kind: "fix",
        text: "Meshing-Verfahren „Reach for the Arcs“ entfernt — die sPSR-Rekonstruktion lieferte prinzipbedingt entartete Netze (625 Dreiecke, >5 mm Abweichung) und ist nicht rettbar",
        scope: "export",
        tickets: ["tihw"],
      },
    ],
  },
  {
    version: "0.6.410",
    entries: [
      {
        kind: "feat",
        text: "Kamera-Blickwinkel (Position, Ausrichtung, Orbit-Ziel, Zoom) wird jetzt im Projekt gespeichert und beim Öffnen/Neuladen wiederhergestellt — statt immer auf die Default-Ansicht zu springen",
        scope: "viewport",
        tickets: ["pxff"],
      },
    ],
  },
  {
    version: "0.6.409",
    entries: [
      {
        kind: "fix",
        text: "Mesher: die Surface-Farbschwelle wirkte kaum — die frühere graduelle, auf den Maximalwert normierte Farbrampe ließ oberflächennahe Samples fast immer grün wirken. Jetzt scharfe 3-Wege-Klassifikation: |SDF| ≤ Schwelle = grün, sonst blau (innen) / rot (außen). Der Schwellwert steuert das Grün nun direkt (kleiner Wert = wenig grün)",
        scope: "debug",
        tickets: ["0ly1"],
      },
    ],
  },
  {
    version: "0.6.408",
    entries: [
      {
        kind: "feat",
        text: "Mesher-Debug erweitert: ein Parameter Surface ± für den Grün-/Surface-Schwellwert der Farbcodierung (|SDF| darunter = grün, darüber Rampe nach blau/rot) und ein Part-opacity-Slider, der den Solid durchscheinend macht — im transparenten Modus werden auch die innenliegenden Normalen gezeigt",
        scope: "debug",
        tickets: ["dbmy"],
      },
    ],
  },
  {
    version: "0.6.407",
    entries: [
      {
        kind: "fix",
        text: "Mesher-Debug: Parameter Band guard in Band width umbenannt und mit Erklär-Tooltip versehen — er steuert die Narrow-Band-Breite der Octree-Unterteilung (nicht die Inside/Outside-Farbtrennung). Oberhalb ~1 gesättigt (1 bis 8 ohne sichtbaren Effekt), unter 1 engere Band, also weniger oberflächennahe Zellen/Normalen",
        scope: "debug",
        tickets: ["dbmy"],
      },
    ],
  },
  {
    version: "0.6.406",
    entries: [
      {
        kind: "fix",
        text: "Menüpunkt Debug ▸ Part-AABB war nach rechts eingerückt (ein unsichtbares EM-Space-Zeichen als Zustands-Platzhalter). Der Eintrag steht jetzt linksbündig wie die Nachbarn; der Aktiv-Zustand zeigt ein nachgestelltes Häkchen",
        scope: "ui",
        tickets: ["cvii"],
      },
    ],
  },
  {
    version: "0.6.405",
    entries: [
      {
        kind: "fix",
        text: "Umschalten Perspektive ⇄ Orthografisch springt nicht mehr: das Framing gleicht die Bildgröße jetzt um den tatsächlichen Blickmittelpunkt (Orbit-Target) ab statt um das Szenen-AABB-Zentrum — bei geschnittenen/versetzten Teilen wichen die auseinander. Zusätzlich erben die beim Umschalten neu erzeugten Kamera-Controls das letzte Orbit-Target, sodass ein Rotieren direkt nach dem Umschalten weiter um denselben Punkt dreht",
        scope: "camera",
        tickets: ["q3bb"],
      },
    ],
  },
  {
    version: "0.6.404",
    entries: [
      {
        kind: "feat",
        text: "Kamera-Pivot wie in Onshape: Beim Start der Rotation wird ein Strahl vom Cursor in die Szene geschossen — trifft er Geometrie, dreht die Ansicht um genau diesen Punkt; trifft er ins Leere, um das Zentrum der sichtbaren Szene. Ersetzt die bisherige AABB-Sticky-Heuristik; man dreht jetzt um das, was unter dem Mauszeiger liegt (Maus + Touch)",
        scope: "camera",
        tickets: ["ohtn"],
      },
    ],
  },
  {
    version: "0.6.403",
    entries: [
      {
        kind: "fix",
        text: "Auto-reload-Off-Unterdrückung robuster gemacht: statt sich auf die Listener-Reihenfolge zu verlassen (stopImmediatePropagation, brach wiederholt), umschließt der HMR-Socket-Patch jetzt Vites eigene message/close-Handler direkt und blendet full-reload-Payloads sowie den Server-Restart-Close aus, solange Auto-reload aus ist",
        scope: "dev",
        tickets: ["y2b8"],
      },
    ],
  },
  {
    version: "0.6.402",
    entries: [
      {
        kind: "fix",
        text: "Feature-Details-Panel hat keinen Schlagschatten mehr — es ist in der Seitenleiste angedockt, kein schwebendes Fenster. Der Schatten bleibt den verschiebbaren/modalen Dialogen vorbehalten",
        scope: "ui",
        tickets: ["35np"],
      },
    ],
  },
  {
    version: "0.6.401",
    entries: [
      {
        kind: "fix",
        text: "Part-AABB-Panel: Schließen-× oben rechts ergänzt und das dekorative cyan-Kästchen vor dem Titel entfernt. Die Anzeige wechselt jetzt korrekt vom Leer-Hinweis auf die Maße des selektierten Parts (zusammen mit dem AABB-Fix ipnx)",
        scope: "debug",
        tickets: ["5egt"],
      },
    ],
  },
  {
    version: "0.6.400",
    entries: [
      {
        kind: "fix",
        text: "Rotier-Pivot lag bei Körpern mit einem Gyroid-Intersect wieder im Ursprung: ein space-filling Werkzeug (Gyroid) meldet eine leere AABB, wodurch der Intersect die Body-AABB fälschlich auf leer kollabierte. Ein unbegrenztes Intersect-Werkzeug schränkt die Box jetzt nicht mehr ein — der Pivot folgt wieder dem sichtbaren Part",
        scope: "kernel",
        tickets: ["ipnx"],
      },
    ],
  },
  {
    version: "0.6.399",
    entries: [
      {
        kind: "fix",
        text: "Zahlenfelder sind jetzt durchgängig eine relaxte, text-basierte Eingabe (NumberInput) statt des nativen type=number: Zwischenzustände beim Tippen (führendes Minus, ein hängender Punkt, ein Dezimalkomma) sind erlaubt, kein Zurückspringen auf 0, kein aufgezwungener Spinner. Ersetzt alle rohen Number-Felder (Sketch-Werkzeuge, SDF-Plane, Mesher, Export, Sketch-Bemaßung)",
        scope: "ui",
      },
    ],
  },
  {
    version: "0.6.398",
    entries: [
      {
        kind: "feat",
        text: "Debug ▸ Part-AABB: blendet die achsenparallele Bounding-Box des selektierten Parts als Wireframe im Viewport ein, plus ein Panel mit Größe (X × Y × Z) und Min/Max-Ecke. Nutzt die analytische, pipeline-gepflegte Body-AABB (eng nach Boolean-Schnitten)",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.397",
    entries: [
      {
        kind: "fix",
        text: "Body-AABB wird jetzt analytisch pro Feature durch die Pipeline gepflegt (wie das SDF): eine Remove-Operation zieht die Bound hinter die vom Werkzeug abgedeckte Schicht zurück, ein Intersect klippt sie auf das Werkzeug. Der Rotier-Pivot kommt aus dieser Body-AABB statt aus einer SDF-Abtastung — sauberer und ohne Sampling-Heuristik",
        scope: "kernel",
        tickets: ["f88u"],
      },
    ],
  },
  {
    version: "0.6.396",
    entries: [
      {
        kind: "fix",
        text: "Sticky-Pivot-Verhalten wiederhergestellt: solange der Part ins Bild passt, folgt der Rotier-Pivot seinem (jetzt korrekten, engen) Mittelpunkt; ist so weit hineingezoomt, dass der Part nicht mehr ganz ins Bild passt, bleibt der Pivot an seiner letzten passenden Stelle statt auf einen Punkt außerhalb des Bildes zu springen",
        scope: "viewport",
        tickets: ["f88u"],
      },
    ],
  },
  {
    version: "0.6.395",
    entries: [
      {
        kind: "fix",
        text: "Rotier-Pivot lag nach Boolean-Schnitten auf dem Zentrum des ursprünglichen Körpers (z.B. der ganzen Kugel) statt auf dem sichtbaren Rest. Der Pivot ist jetzt der Mittelpunkt der engen AABB des tatsächlich gerenderten Solids (aus dem SDF-Feld abgetastet), nicht mehr der konservativen CSG-Hülle",
        scope: "viewport",
        tickets: ["f88u"],
      },
    ],
  },
  {
    version: "0.6.394",
    entries: [
      {
        kind: "fix",
        text: "Rotieren dreht jetzt immer um den Mittelpunkt der AABB des angezeigten Parts — die frühere „sticky pivot“-Logik (Pivot bleibt an der letzten passenden Stelle, wenn der Körper nicht ganz ins Bild passt) ist entfallen. Auch nach Verschieben/Zoomen zentriert ein Rotieren wieder auf den Part",
        scope: "viewport",
        tickets: ["f88u"],
      },
    ],
  },
  {
    version: "0.6.393",
    entries: [
      {
        kind: "fix",
        text: "Einstellung „Auto-reload on code change = off“ wird wieder respektiert: Vite 8 verschluckt (via Promise.allSettled) den Fehler, mit dem wir bisher den Full-Reload abbrachen — die Seite lud trotzdem neu (und verwarf das ungespeicherte Projekt). Neu wird die Vite-„full-reload“-Nachricht direkt am HMR-Socket abgefangen, bevor Vite sie verarbeitet",
        scope: "dev",
        tickets: ["0u31"],
      },
    ],
  },
  {
    version: "0.6.392",
    entries: [
      {
        kind: "feat",
        text: "Mesher-Debug-Ansicht verfeinert: die Normalen-Linien sind jetzt in der SDF-Farbe kodiert (innen/Oberfläche/außen) und haben eine einstellbare Stroke-Width; der Zell-Mittelpunkt ist ein runder Punkt (Durchmesser 3× Stroke-Width) in derselben Farbe. Neu: Checkbox „Near surface only“ mit Distanz-Eingabe blendet nur Octree-Zellen nahe der Oberfläche ein",
        scope: "debug",
        tickets: ["lbb3"],
      },
    ],
  },
  {
    version: "0.6.391",
    entries: [
      {
        kind: "fix",
        text: "Kamera lässt sich jetzt per Touch bedienen: ein Finger rotiert (nach kurzer Bewegung, ein Tippen selektiert weiterhin), zwei Finger zoomen (Pinch) und schieben (Pan). Vorher fingen Browser die Gesten ab und zoomten/verschoben die ganze Seite — die OnshapeControls hatten keine Touch-Verarbeitung, und dem Viewport fehlte touch-action:none samt user-scalable=no",
        scope: "viewport",
        tickets: ["0m1l"],
      },
    ],
  },
  {
    version: "0.6.390",
    entries: [
      {
        kind: "feat",
        text: "„Report Issue…“ ist jetzt auch im Haupt-Hamburger-Menü (neben Settings) — so lassen sich Fehler/Wünsche auch im nicht eingeloggten Zustand (Dev-Server) melden, nicht nur über das Benutzermenü",
        scope: "ui",
      },
    ],
  },
  {
    version: "0.6.389",
    entries: [
      {
        kind: "fix",
        text: "App-Menü (Hamburger) lief auf schmalen/mobilen Bildschirmen rechts aus dem Viewport (Shortcuts und Submenu-Pfeil abgeschnitten). Auf Telefonen ist die Dropdown-Liste jetzt am rechten Rand des Buttons verankert (öffnet nach links) und maximal viewport-breit — sie bleibt vollständig sichtbar",
        scope: "ui",
      },
    ],
  },
  {
    version: "0.6.388",
    entries: [
      {
        kind: "feat",
        text: "Neues Debug-Werkzeug „Mesher“ (Menü Debug ▸ Mesher…) als Gerüst für ein eigenes Meshing-Verfahren. Schritt 1: ein adaptiver Narrow-Band-Octree — jeder Blattzell-Mittelpunkt wird gegen SDF und Gradient ausgewertet und als Punkt gezeichnet (Farbe kodiert die SDF: innen blau, außen rot, an der Oberfläche grün) mit einer kurzen Linie entlang der Normale. Parameter: Max-Tiefe, Band-Breite, Normalen-Länge, Punktgröße; Zellzahl-Anzeige; Einstellungen in localStorage",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.387",
    entries: [
      {
        kind: "fix",
        text: "Der Wechsel des Render-Shaders (WebGPU ⇄ WebGL) setzt die Kamera nicht mehr zurück: der Wechsel remountet den Canvas mit einer frischen Kamera auf der Standard-Ansicht — jetzt wird die Kamera-Pose (Position, Blickrichtung, Zoom, Orbit-Pivot) über den Wechsel hinweg erhalten",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.386",
    entries: [
      {
        kind: "fix",
        text: "SDF-Plane wird jetzt vom raymarching-Solid korrekt verdeckt und wandert nicht mehr blickwinkelabhängig: das Overlay-Quad lag auf dem Kanten-Layer, dessen Pass die Solid-Tiefe für den Röntgen-Effekt der B-Rep-Kanten löscht — dadurch schwebte die Ebene über dem Körper und zeigte einen Parallaxe-Versatz. Jetzt liegt sie (wie eine Datum-Ebene) auf dem Datum-Layer, der vor dem Tiefen-Löschen gezeichnet wird, sodass die grüne Null-Linie exakt auf der Körperoberfläche sitzt. Die SDF-Formeln (auch Gyroid) waren identisch — es war kein Feld-, sondern ein reiner Tiefen-/Rendering-Versatz. Nebenbei rendert die Ebene jetzt auch im WebGL-Modus",
        scope: "debug",
        tickets: ["iwgq"],
      },
    ],
  },
  {
    version: "0.6.385",
    entries: [
      {
        kind: "feat",
        text: "SDF-Plane-Dialog merkt sich seine Einstellungen (Punkt, Normale, Größe, Auflösung, Iso-Bänder, Step) in localStorage — sie überleben Schließen/Öffnen und Seiten-Reload",
        scope: "debug",
        tickets: ["iepg"],
      },
    ],
  },
  {
    version: "0.6.384",
    entries: [
      {
        kind: "feat",
        text: "SDF-Plane-Dialog: die Schrittweite der Punkt-/Normalen-XYZ-Felder (Pfeiltasten/Spinner) ist über ein eigenes „Step“-Feld einstellbar — so lässt sich die Ebene grob oder fein durch das Modell schieben",
        scope: "debug",
        tickets: ["iepg"],
      },
    ],
  },
  {
    version: "0.6.383",
    entries: [
      {
        kind: "feat",
        text: "SDF-Plane-Debug-Dialog erweitert: einstellbare Iso-Band-Anzahl (feinere Höhenlinien) und einstellbare Sampling-Auflösung der Farbkodierung; die Punkt-/Normalen-Felder sind breiter und ohne Labels; und der numerische SDF-Wert an der auf die Ebene projizierten Mausposition wird live im Dialog angezeigt",
        scope: "debug",
        tickets: ["iepg"],
      },
    ],
  },
  {
    version: "0.6.382",
    entries: [
      {
        kind: "feat",
        text: "Neues Meshing-Verfahren MACET (Dietrich/Scheidegger, IEEE TVCG 2009): behält die Marching-Cubes-33-Topologie, verschiebt aber jede aktive Gitterkante so, dass sie senkrecht auf der Isofläche steht (Gradient- + Tangential-Transform mit dem analytischen ∇f) und findet den Schnitt per Bisektion neu — beseitigt schlanke/entartete Dreiecke und bleibt per Konstruktion watertight. Etwas langsamer (Nullstellensuche pro Kante). Verifiziert: Kugel geschlossen und ohne entartete Dreiecke",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.381",
    entries: [
      {
        kind: "feat",
        text: "Drei neue Meshing-Verfahren im Export-Dialog: Marching Cubes 33 (Lewiner 2003 — topologisch korrekt, manifold & watertight für beliebige Felder, gute Wahl für Gyroid/TPMS), Dual Marching Cubes (Nielson 2004 — quad-dominant, wenige Slivers) und Extended Marching Cubes (Kobbelt 2001 — setzt in Zellen mit scharfer CSG-Kante einen QEF-Feature-Punkt und stellt so knackige Boolean-Kanten wieder her). MC33 & DMC sind per Konstruktion geschlossen (verifiziert auf Kugel und CSG-Schnitt)",
        scope: "export",
      },
      {
        kind: "fix",
        text: "Release-Notes-Datei ließ sich nicht mehr typprüfen: im vorigen SDF-Plane-Eintrag schloss ein ASCII-Anführungszeichen den String vorzeitig — auf ein typografisches „ “ korrigiert",
        scope: "build",
      },
    ],
  },
  {
    version: "0.6.380",
    entries: [
      {
        kind: "feat",
        text: "Debug-Werkzeug: Menü „Debug ▸ SDF-Plane…“ öffnet einen kleinen Dialog (Punkt + Normale + Größe). Solange er offen ist, wird eine Schnitt-Ebene im Raum angezeigt, die das exakte SDF-Feld der Szene farbcodiert (innen/außen, Iso-Distanz-Bänder, Null-Oberflächen-Linie) — zum Debuggen von Meshing-/Rasterungs-Artefakten direkt im Feld",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.6.379",
    entries: [
      {
        kind: "fix",
        text: "Surface-Nets-Export reißt keine Löcher mehr an Boolean-Schnittkanten: der QEM-Dezimierer der Bibliothek (baby_shark) zerriss einen an CSG-Nähten leicht non-manifolden Mesh in hunderte offene Kanten (statt zu dezimieren). Neu erkennt der Export das und weicht dort auf den loch-sicheren eigenen Dezimierer aus — der Mesh bleibt geschlossen und wird zudem stärker reduziert",
        scope: "export",
        tickets: ["8l06"],
      },
    ],
  },
  {
    version: "0.6.378",
    entries: [
      {
        kind: "fix",
        text: "Export-Auflösung wird nicht mehr fälschlich gekappt, wenn ein Körper per Boolean-Intersect (z. B. ∩ Box) auf wenige Millimeter beschnitten ist: die Schnitt-AABB verengt sich jetzt auf den tatsächlichen Überlappungsbereich, sodass der beschnittene Bereich bei der gewünschten Zellgröße gemesht wird — an den Boolean-Kanten verschwinden dadurch die zellgroßen Zacken/Löcher (die SDF dort ist sauber, es war reine Unterauflösung)",
        scope: "export",
        tickets: ["8l06"],
      },
    ],
  },
  {
    version: "0.6.377",
    entries: [
      {
        kind: "fix",
        text: "Standard-Export (Adaptive) verschifft beim Gyroid keine Löcher mehr: der uniforme Fallback lieferte auf stark nicht-lipschitz Feldern bis zu 1471 offene Kanten + einen 17 mm daneben liegenden Ausreißer-Vertex. Neu greift ein garantiert lochfreies Marching-Tets-Netz, das den defektärmeren Mesh behält",
        scope: "export",
        tickets: ["9ocx"],
      },
    ],
  },
  {
    version: "0.6.376",
    entries: [
      {
        kind: "feat",
        text: "Export-Dialog zeigt bei den dichten Meshern schon vor dem Meshen die erreichbare Zellgrößen-Untergrenze (Speicher-Budget) und eine Dreiecks-Abschätzung — so ist der Aufwand vorab sichtbar. Aktualisiert sich live beim Ändern von Zellgröße/Verfahren",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.375",
    entries: [
      {
        kind: "fix",
        text: "Marching Cubes (robust) exportiert wieder watertight — ein Vorzeichenwechsel wurde je nach Zelle von unterschiedlichen Ecken interpoliert, sodass geteilte Kreuzungspunkte nicht verschmolzen (eine Kugel verlor 14.672 offene Kanten). Gyroid & Co. sind damit watertight ohne Löcher; an CSG-Schnittnähten bleiben wenige Non-Manifold-Kanten",
        scope: "export",
        tickets: ["9ocx"],
      },
    ],
  },
  {
    version: "0.6.374",
    entries: [
      {
        kind: "feat",
        text: "Dev-Modus: automatischer Login als Seed-Admin (admin@local) beim Start des `vite dev`-Servers — Account-Features sind ohne manuelles Einloggen erreichbar. Nur im Dev-Browser aktiv (nicht in Prod, Tests oder e2e); Zugangsdaten via VITE_DEV_AUTOLOGIN_EMAIL/PASSWORD überschreibbar",
        scope: "account",
        tickets: ["oyx8"],
      },
    ],
  },
  {
    version: "0.6.373",
    entries: [
      {
        kind: "fix",
        text: "Fenster-Resize setzt die Kamera nicht mehr zurück — die aktuelle Ansicht (Position, Ausrichtung, Zoom) bleibt beim Ändern der Fenstergröße erhalten",
        scope: "viewport",
        tickets: ["flzq"],
      },
    ],
  },
  {
    version: "0.6.372",
    entries: [
      {
        kind: "fix",
        text: "Export-Dialog aufgeräumt: Spinner während des Meshens; die Body-Report-Tabelle ist transponiert (Attribute als Zeilen, ein Body je Spalte) und läuft nicht mehr horizontal über — die lange Mesh-Bezeichnung umbricht jetzt in der Zelle; die Vorschau-Einstellungen (Crossfade SDF↔Mesh, Wireframe) werden in localStorage gespeichert",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.371",
    entries: [
      {
        kind: "feat",
        text: "Zwei bisher nur im Mesh-Viewer verfügbare Verfahren sind jetzt auch für den Export wählbar: Manifold DC (viewport) — das uniforme Manifold Dual Contouring des Viewports mit Planar-Face-Simplifikation; und Reach for the Arcs (Sellán et al. 2024) — Kugeln → Tangentenpunkte → Screened-Poisson-Rekonstruktion, robust auf unscharfen/unvollständigen Feldern (interne Auflösung gedeckelt). Damit deckt der Export-Dialog alle vorhandenen Rust-Mesher ab",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.370",
    entries: [
      {
        kind: "feat",
        text: "Zwei weitere Export-Meshing-Verfahren im Dropdown: Uniform Dual Contouring (QEF) — dichtes Gitter mit QEF-Vertex-Platzierung: watertight by construction UND kantentreu (behält scharfe Kanten), nicht-1-Lipschitz-sicher, dann QEM-Decimation (der Allrounder für scharfkantige Körper). Und Marching cubes (robust) — Marching Tetrahedra: ambiguitätsfreie Fälle liefern watertight + 2-manifold by construction für JEDES Feld (die robuste Basislinie, u. a. watertight beim Gyroid, wo das Intervall-DC ein paar non-manifold-Kanten ließ). Die Report-Zeile zeigt jetzt das Verfahren als Klartext-Label",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.369",
    entries: [
      {
        kind: "feat",
        text: "Export-Vorschau: ein Schieberegler blendet das Mesh-Solid transparent über den SDF-gerenderten Körper (Crossfade von SDF ↔ Mesh), und eine Checkbox zeigt das Mesh-Wireframe. So lassen sich Mesh und Original direkt überlagern und die Tessellierung inspizieren, solange der Export-Dialog offen ist",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.368",
    entries: [
      {
        kind: "feat",
        text: "Viertes Export-Meshing-Verfahren: „Interval-octree DC (fidget)“. Übersetzt das Körperfeld in einen fidget-Ausdrucksbaum und meshet es per Intervall-Arithmetik-Octree + Manifold Dual Contouring: die Zell-Unterteilung wird per Intervall-Schranken auf dem echten Ausdruck zertifiziert (keine Lipschitz-Annahme → topologie-sicher für nicht-1-Lipschitz-Felder) und QEF bewahrt scharfe Kanten ohne B-Rep. Watertight auf glatten closed-form-Körpern und sehr genau (p95 ~0,01 mm); an scharfen/dichten Feature-Kanten hinterlässt das adaptive DC ein paar non-manifold-Kanten (Report zeigt es) — für garantiert watertight Surface Nets nutzen. Nur closed-form-Körper; Sketch-/Daten-Carrier fallen auf Surface Nets zurück",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.367",
    entries: [
      {
        kind: "fix",
        text: "Dialoge (Feature-, Export-, Report-Dialog, Measure-Panel) heben sich mit einem etwas kräftigeren, zweilagigen Schlagschatten (neues --shadow-dialog-Token) klarer vom Viewport ab; der Feature-Dialog hatte bisher gar keinen Schatten",
        scope: "ui",
      },
    ],
  },
  {
    version: "0.6.366",
    entries: [
      {
        kind: "feat",
        text: "Export-Dialog hat ein Dropdown für das Meshing-Verfahren. Neben dem bisherigen (Adaptive Dual Contouring) gibt es Surface Nets + Decimation: ein dichtes Gitter wertet jede Zelle aus (keine Lipschitz-Annahme) und findet so die Topologie nicht-1-Lipschitz-Geometrien (Gyroid, TPMS, dichte Helix-Windungen) zuverlässig; anschließend dezimiert QEM-Kantenkollaps das Mesh watertight-erhaltend. Zwei Umsetzungen wählbar: „library“ (fast-surface-nets + baby_shark) und „built-in“ (eigene, abhängigkeitsfreie Implementierung). Der Report zeigt die Dreiecks-Reduktion (dicht → dezimiert)",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.365",
    entries: [
      {
        kind: "feat",
        text: 'Export-Dialog zeigt eine Solid-Vorschau des gemeshten Objekts: nach „Mesh" wird im Viewport das exakte Export-Mesh (flat-shaded) statt des normalen Renderers angezeigt — solange der Dialog offen ist. Der Dialog ist jetzt non-modal und am Titelbalken verschiebbar, sodass man frei um das Mesh herum navigieren kann; beim Schließen übernimmt wieder der Original-Renderer',
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.364",
    entries: [
      {
        kind: "fix",
        text: 'Export-Dialog zweistufig: erst Zellgröße setzen und mit „Mesh" bestätigen, dann meshen und Speichern — das (bei Gyroiden/Blends teure) Meshing startet nicht mehr automatisch beim Öffnen und nicht bei jedem Tastendruck; Zellgröße ändern verwirft das alte Mesh und verlangt erneutes Bestätigen',
        scope: "export",
        tickets: ["0xir"],
      },
    ],
  },
  {
    version: "0.6.363",
    entries: [
      {
        kind: "feat",
        text: "Export-Meshing-Restposten: getrimmte Kugel-Patches tessellieren parametrisch (synthetisches Halbkreis-Profil speist die Rotations-Maschinerie), Selbstschnitt-Prüfung (Dreieck-Dreieck über Grid-Hash) und Dünnwand-Erkennung (Sub-Zellen-Features zwischen den Abtastebenen) im Export-Report mit Dialog-Warnungen; breiteres Narrow-Band für nicht-Lipschitz-Felder",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.362",
    entries: [
      {
        kind: "feat",
        text: "Adaptives generisches Export-Meshing (Manifold Dual Contouring nach Schaefer/Ju/Warren auf Narrow-Band-Octree): topologie-erhaltendes Vertex-Clustering + konformes Sag-Refinement + Geisterkanten-Filter fürs Pinning — die hohle Vase exportiert mit 100k statt 1,5 Mio. Dreiecken, watertight, 0,07 mm Abweichung; Blend-Körper (SDF-only) 2.100 Dreiecke bei 0,05 mm",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.361",
    entries: [
      {
        kind: "fix",
        text: "Generisches Export-Meshing gehärtet (Meshing Phase 1a/1b): MDC-Quad-Winding an scharfen Kanten an der Wurzel gefixt (geometrische Orientierung statt Achsen-Tabelle), Simplifikation deterministisch (kein Watertight-Würfeln mehr zwischen Läufen) mit sauberem Loch-Bridging, und B-Rep-Kanten-Pinning: Kanten-Vertices landen exakt auf den echten Kantenkurven",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.360",
    entries: [
      {
        kind: "feat",
        text: "Parametrischer Export für Revolve-Körper: Spline-/Arc-Profile (RevolvedProfile-Carrier) tessellieren direkt aus dem B-Rep — die Vase exportiert mit 9.700 statt 1,5 Mio. Dreiecken; Kuppeln (Profil endet auf der Achse) über Pol-Fächer; generisches Sag-Refinement ersetzt die Winkel-Heuristik",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.359",
    entries: [
      {
        kind: "feat",
        text: "Parametrischer Mesh-Export (Meshing Phase 2): analytisch weavbare Körper (Box/Zylinder/Kugel/Kegel/Torus-Booleans) tessellieren direkt aus dem B-Rep — watertight by construction, exakte Kanten, drastisch weniger Dreiecke (Zylinder: 252 statt 28.000); automatischer Fallback aufs generische Meshing, Report zeigt die Route pro Körper",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.358",
    entries: [
      {
        kind: "feat",
        text: "STL/3MF-Export komplett neu: eigener toleranz-getriebener Mesh-Pass (Zellgröße einstellbar, unabhängig von der Viewport-Auflösung) mit Watertight-Validierung, Orientierungs-Reparatur und Qualitätsreport im neuen Export-Dialog",
        scope: "export",
      },
    ],
  },
  {
    version: "0.6.356",
    entries: [
      {
        kind: "fix",
        scope: "sketch",
        text: "Einen selektierten Linien-Endpunkt oder eine Rechteck-Ecke zu ziehen verschiebt jetzt nur diesen Griff (bzw. hält die Gegenecke fest), statt die ganze Form zu verschieben — der Endpunkt ist ein eigenes Punkt-Entity, das der Drag-Planer nun als Griff der selektierten Kurve erkennt",
      },
    ],
  },
  {
    version: "0.6.355",
    entries: [
      {
        kind: "fix",
        scope: "kernel",
        text: "Ein NaN aus dem Sketch-Solver kann den Kernel nicht mehr abstürzen lassen — der B-Rep-Weave prüft die Geometrie vorab und liefert bei nicht-endlichen Koordinaten ein leeres B-Rep statt zu paniken",
      },
    ],
  },
  {
    version: "0.6.354",
    entries: [
      {
        kind: "fix",
        text: "STEP-Export und -Import können nicht mehr gegenseitig ihre Antworten kapern (Antworten werden per Request-Id zugeordnet statt per Nachrichtentyp); Fehler aus den Mesh-/Weave-/STEP-Workern werden gemeldet statt als leeres Ergebnis maskiert",
        scope: "step",
      },
    ],
  },
  {
    version: "0.6.353",
    entries: [
      {
        kind: "fix",
        text: "Picking/Selektion kullt Punkte hinter der Kamera jetzt korrekt (Box-Select, Kanten-Snap, Vertex-Pick, Zoom-to-fit-Sichtbarkeit) — die NDC-Tiefenpr\u00fcfung nutzte die WebGL- statt der WebGPU-Clip-Space-Konvention",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.352",
    entries: [
      {
        kind: "fix",
        text: "SDF VM rendert Körper jetzt auch in der orthografischen Ansicht (Default) geshadet — der Ray-Origin wurde mit WebGL- statt WebGPU-Clip-Space rekonstruiert und lag ~far hinter der Kamera",
        scope: "sdf-vm",
        tickets: ["yl0p"],
      },
    ],
  },
  {
    version: "0.6.351",
    entries: [
      {
        kind: "fix",
        text: "Überstehende/überschießende Linien verhindern keine geschlossene Fläche mehr — ein Rechteck mit Fahnenmast-Überstand wird korrekt grau gefüllt (und ist extrudierbar)",
        scope: "sketch",
        tickets: ["mdye"],
      },
    ],
  },
  {
    version: "0.6.349",
    entries: [
      {
        kind: "feat",
        text: "Benutzerkonten mit Login/Logout — FieldDraft ist jetzt mehrbenutzerfähig",
        scope: "accounts",
      },
      {
        kind: "feat",
        text: "Registrierung nur per Einladungs-Link (Invite-only)",
        scope: "accounts",
      },
      {
        kind: "feat",
        text: "Anmeldepflicht in der ausgelieferten App; die Anmeldung bleibt über Reloads erhalten",
        scope: "accounts",
      },
      {
        kind: "feat",
        text: "Einladungen im Admin-Bereich: Name, E-Mail und Einladungstext, mit teilbarem Registrierungs-Link",
        scope: "invites",
      },
      {
        kind: "feat",
        text: "Selbst-Registrierung über den Invite-Link mit vorausgefüllter E-Mail-Adresse",
        scope: "invites",
        tickets: ["dqmh"],
      },
      {
        kind: "feat",
        text: "Cloud-Projekte: Browser für eigene (privat/öffentlich) Projekte mit Filter — anlegen, umbenennen, duplizieren, löschen und Sichtbarkeit umschalten",
        scope: "projects",
      },
      {
        kind: "feat",
        text: "Öffentliche Galerie: freigegebene Projekte anderer ansehen und als Kopie öffnen",
        scope: "projects",
      },
      {
        kind: "feat",
        text: "Projekt-Titel in der Topbar per Klick editierbar",
        scope: "projects",
        tickets: ["yoyz"],
      },
      {
        kind: "feat",
        text: "Issue-Tracker im GitHub-Issues-Stil: Fehler und Wünsche per „Report Issue“ melden (inkl. Screenshot und Projekt), Issues-Liste mit Filter „nur meine“ und Status, Detailansicht mit Kommentar-Thread",
        scope: "issues",
      },
      {
        kind: "feat",
        text: "Admin: Benutzerverwaltung (Sperren/Entsperren) und ein Hinweis-Glöckchen für sicherheitsrelevante Ereignisse",
        scope: "admin",
      },
      {
        kind: "feat",
        text: "Neue Navigation: persistente Topbar, Tab-Übersicht (Projekte/Issues/Invites/Users) und ein 👤-Benutzermenü mit „Report Issue“",
        scope: "ui",
      },
      {
        kind: "fix",
        text: "Perf-Overlay togglet auf F9 (statt F2) und hat jetzt einen Schließen-Button",
        scope: "ui",
        tickets: ["f5d8"],
      },
    ],
  },
  {
    version: "0.6.348",
    entries: [
      {
        kind: "fix",
        text: "Mate-Connector zeigt kein Translate/Rotate/Scale-Gizmo mehr — er wird über Anker & Offset positioniert, nicht per Freie-Transform",
        scope: "mate",
        tickets: ["zcwn"],
      },
    ],
  },
  {
    version: "0.6.347",
    entries: [
      {
        kind: "feat",
        text: "Mate-Connector-Realignment: Origin entlang eigener X/Y/Z-Achsen verschieben (Offset)",
        scope: "mate",
      },
      {
        kind: "feat",
        text: "Mate-Connector zwischen zwei Elementen platzieren (2. Anker → Mittelpunkt)",
        scope: "mate",
      },
      {
        kind: "feat",
        text: "Mate-Connector einem Owner-Part zuordnen",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.346",
    entries: [
      {
        kind: "feat",
        text: "Extrude: mehrere coplanare Flächen im Profiles-Feld auswählen — Profiles ist jetzt ein Multi-Picker (Flächen sammeln sich an), das separate Faces-Feld entfällt",
        scope: "extrude",
        tickets: ["k280"],
      },
    ],
  },
  {
    version: "0.6.345",
    entries: [
      {
        kind: "fix",
        text: "Versionsnummer in der Titelzeile als Link erkennbar (dezente Unterstreichung) — Klick öffnet die Release Notes",
        scope: "release-notes",
      },
    ],
  },
  {
    version: "0.6.343",
    entries: [
      {
        kind: "feat",
        text: "Release Notes — pro Version erledigte Features & Fixes, geöffnet übers Menü oder per Klick aufs Versions-Badge, mit Filter nach Stichwort/Ticket",
        scope: "release-notes",
      },
    ],
  },
  {
    version: "0.6.342",
    entries: [
      {
        kind: "feat",
        text: "Seitenfläche als Profil extrudieren — qFace im Profiles-Feld",
        scope: "extrude",
        tickets: ["20fc"],
      },
    ],
  },
  {
    version: "0.6.341",
    entries: [
      {
        kind: "fix",
        text: '„Profile half x/y" ausblenden + klare Fehlermeldung bei Face-Profil',
        scope: "extrude",
        tickets: ["r2zx"],
      },
    ],
  },
  {
    version: "0.6.340",
    entries: [
      {
        kind: "feat",
        text: "Constraint-Tool-Kette — alle nacheinander geklickten Entities verketten",
        scope: "sketch",
        tickets: ["32az"],
      },
    ],
  },
  {
    version: "0.6.339",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Tiefenänderung aktualisiert den Solid",
        scope: "sdf-deferred",
        tickets: ["c9ic"],
      },
    ],
  },
  {
    version: "0.6.338",
    entries: [
      {
        kind: "fix",
        text: "screen-space Stroke-Width statt 3D-Röhren",
        scope: "mate-gizmo",
      },
    ],
  },
  {
    version: "0.6.337",
    entries: [
      {
        kind: "fix",
        text: 'leere Szene nach „New" sofort räumen',
        scope: "new-project",
        tickets: ["vy3t"],
      },
    ],
  },
  {
    version: "0.6.336",
    entries: [
      {
        kind: "feat",
        text: "Shortcuts Ctrl+M / K / Shift",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.335",
    entries: [
      {
        kind: "feat",
        text: "CSYS-Redesign — Kante=Primärachse, Reorient-Toggle, Anker ohne Pflicht-Fläche",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.334",
    entries: [
      {
        kind: "fix",
        text: "Gizmo + Inferenz auf OVERLAY_LAYER — im Deferred-Modus sichtbar",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.333",
    entries: [
      {
        kind: "feat",
        text: "Connection-Point-Inferenz + Vertex/Kanten-Anker",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.332",
    entries: [
      {
        kind: "feat",
        text: "Mate-Connector-Gizmo — Triade + Drehkreis + gefüllter Sektor",
        scope: "mate",
      },
    ],
  },
  {
    version: "0.6.331",
    entries: [
      {
        kind: "fix",
        text: "SS-CSG-Tiefe ortho-tauglich — Kugel im Box-Inneren wieder verdeckt",
        scope: "ss-csg",
        tickets: ["sa0k"],
      },
    ],
  },
  {
    version: "0.6.330",
    entries: [
      {
        kind: "fix",
        text: "WebGL Screen-Space-CSG nicht mehr dunkel — Live-Lichtrig + ACES/sRGB-Parität",
        scope: "ss-csg",
        tickets: ["mlv3"],
      },
    ],
  },
  {
    version: "0.6.328",
    entries: [
      {
        kind: "fix",
        text: "Kanten sofort beim Laden zeigen — Demand-Mode-Repaint nach Async-Weave-Import",
        scope: "brep-overlay",
        tickets: ["1do9", "zhfy"],
      },
    ],
  },
  {
    version: "0.6.327",
    entries: [
      {
        kind: "fix",
        text: "Durchgangsloch in box-minus-Bohrung sichtbar — Box-Leaf marschieren bei Nicht-Union-Body",
        scope: "sdf-deferred",
        tickets: ["87bf"],
      },
    ],
  },
  {
    version: "0.6.326",
    entries: [
      {
        kind: "fix",
        text: "Körper rendern wieder — setLayout-Eval revertiert",
        scope: "sdf-vm",
        tickets: ["o6ia"],
      },
    ],
  },
  {
    version: "0.6.325",
    entries: [
      {
        kind: "feat",
        text: "Thicken als Toolbar-Tool — Flächen anklicken wie Fillet",
        scope: "thicken",
      },
    ],
  },
  {
    version: "0.6.324",
    entries: [
      {
        kind: "fix",
        text: "Feature-Kind klassifizieren — Detail-Panel zeigt Thicken-Params",
        scope: "thicken",
        tickets: ["iz3p"],
      },
    ],
  },
  {
    version: "0.6.323",
    entries: [
      {
        kind: "fix",
        text: "VM-Shader-Compile von >1min auf ~210ms — SDF-Eval als eine WGSL-Funktion via setLayout",
        scope: "sdf-vm",
        tickets: ["21ws"],
      },
    ],
  },
  {
    version: "0.6.322",
    entries: [
      {
        kind: "fix",
        text: "Durchgangsloch ortho see-through — March-Ray ortho-aware",
        scope: "sdf-deferred",
        tickets: ["g2ox"],
      },
    ],
  },
  {
    version: "0.6.321",
    entries: [
      {
        kind: "fix",
        text: '"(experimentell)" aus Screen-Space-CSG-Label entfernen',
        scope: "settings",
      },
    ],
  },
  {
    version: "0.6.320",
    entries: [
      {
        kind: "feat",
        text: "UI-Tab in Gruppen ordnen + Screen-Space-CSG nur in WebGL zeigen",
        scope: "settings",
      },
    ],
  },
  {
    version: "0.6.319",
    entries: [
      {
        kind: "fix",
        text: "DC-Boolean-Mesh flach shaden — Schmier-Dreiecke an Hartkanten weg",
        scope: "webgl",
        tickets: ["dkaa"],
      },
    ],
  },
  {
    version: "0.6.318",
    entries: [
      {
        kind: "fix",
        text: "Körper wieder voll rendern statt gesliced — Single-Union-March wiederhergestellt",
        scope: "sdf",
        tickets: ["6yin"],
      },
    ],
  },
  {
    version: "0.6.317",
    entries: [
      {
        kind: "feat",
        text: "edit boolean op + target on primitives",
        scope: "feature-dialog",
      },
    ],
  },
  {
    version: "0.6.316",
    entries: [
      {
        kind: "feat",
        text: '"View Normal to Face" Kontextmenü-Eintrag für planare Flächen (Ticket z0vk)',
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.315",
    entries: [
      {
        kind: "feat",
        text: "Perspektive-Schalter in Settings; Default = Orthografisch (Ticket m62b)",
        scope: "camera",
      },
    ],
  },
  {
    version: "0.6.314",
    entries: [
      {
        kind: "feat",
        text: "One-sided + New defaults, render created body solid",
        scope: "extrude",
        tickets: ["yx5h"],
      },
    ],
  },
  {
    version: "0.6.313",
    entries: [
      {
        kind: "fix",
        text: "separate Bodies nicht mehr implizit boolesch (min-Union) rendern — Nearest-Hit pro Body",
        scope: "sdf",
        tickets: ["deeg", "9ljc"],
      },
    ],
  },
  {
    version: "0.6.312",
    entries: [
      {
        kind: "fix",
        text: "Async-Weave-Import löst Re-Render aus — frisch erzeugte Körper zeigen sofort BRep-Kanten",
        scope: "weaver",
      },
    ],
  },
  {
    version: "0.6.311",
    entries: [
      {
        kind: "fix",
        text: "Snap-Radius zoom-unabhängig (Pixel statt fix-mm) — Polygon schließt wieder",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.310",
    entries: [
      {
        kind: "fix",
        text: "SS-CSG-Pass blockierte Szenen-Render — Datum/Sketch wieder sichtbar + Render-Labels",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.309",
    entries: [
      {
        kind: "fix",
        text: "Nicht-konvexer Subtrahend (Torus) — 2-Layer-Depth-Peel gegen Durchscheinen (Ticket yt4o)",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.308",
    entries: [
      {
        kind: "feat",
        text: "Schnittflächen-Shading an Szenen-Lichtrig angleichen (Parität)",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.307",
    entries: [
      {
        kind: "feat",
        text: "Intersection (A∩B) im Screen-Space-CSG (Stage 1)",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.306",
    entries: [
      {
        kind: "feat",
        text: "Geshadetes A−B mit G-Buffer + Tiefen-Komposition (Stage 1)",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.305",
    entries: [
      {
        kind: "feat",
        text: "Reine Unions parametrisch rendern statt lumpiges DC-Mesh",
        scope: "webgl",
      },
    ],
  },
  {
    version: "0.6.304",
    entries: [
      {
        kind: "feat",
        text: "Screen-Space-CSG Stage-1 (opt-in, experimentell) — Achse 3 Phase 3",
        scope: "webgl-csg",
      },
    ],
  },
  {
    version: "0.6.303",
    entries: [
      {
        kind: "feat",
        text: "Graceful Boolean-Seed-Weave statt All-or-Nothing-Octree-Fallback",
        scope: "brep-seed",
      },
    ],
  },
  {
    version: "0.6.302",
    entries: [
      {
        kind: "fix",
        text: "Lipschitz-Guard mit march_relax skalieren — Revolve-Union nicht mehr degeneriert",
        scope: "brep-octree",
      },
    ],
  },
  {
    version: "0.6.301",
    entries: [
      {
        kind: "feat",
        text: 'ClaudeDraft "open .fdraft" Tab-Loader (?project=)',
        scope: "deep-link",
      },
    ],
  },
  {
    version: "0.6.300",
    entries: [
      {
        kind: "fix",
        text: "Report-Window oben-links unter der Toolbar verankern",
        scope: "report-dialog",
      },
    ],
  },
  {
    version: "0.6.299",
    entries: [
      {
        kind: "fix",
        text: "Identische exakte Meshes + Z-Offset statt Inflation",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.298",
    entries: [
      {
        kind: "fix",
        text: "Konkave Speichen-Wellen per Normal-Offset statt Uniform-Scale",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.297",
    entries: [
      {
        kind: "feat",
        text: "marchRelax auch im VM-Raymarch anwenden",
        scope: "sdf-vm",
      },
    ],
  },
  {
    version: "0.6.296",
    entries: [
      {
        kind: "fix",
        text: "Hover gibt nicht mehr die noch gerenderten Basis-Linien frei",
        scope: "sketch-overlay",
      },
    ],
  },
  {
    version: "0.6.295",
    entries: [
      {
        kind: "fix",
        text: "Hover-Edge/Face-Geometrie gedeferrt freigeben — sdf-vm Buffer-Race",
        scope: "overlays",
      },
    ],
  },
  {
    version: "0.6.294",
    entries: [
      {
        kind: "feat",
        text: "Zoom/Dolly zum Mauscursor hin",
        scope: "camera",
      },
    ],
  },
  {
    version: "0.6.293",
    entries: [
      {
        kind: "feat",
        text: "Standard-Ansichten framen die Bounding-Sphere statt Ursprung",
        scope: "viewcube",
      },
    ],
  },
  {
    version: "0.6.292",
    entries: [
      {
        kind: "feat",
        text: "Boden-Gitter entfernt",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.291",
    entries: [
      {
        kind: "feat",
        text: "Onshape-Style unbegrenzte Rotation statt OrbitControls",
        scope: "camera",
      },
    ],
  },
  {
    version: "0.6.290",
    entries: [
      {
        kind: "fix",
        text: "nicht-degeneriertes Light-Target + per-Frame View→World-Rotation",
        scope: "webgl-lights",
      },
    ],
  },
  {
    version: "0.6.289",
    entries: [
      {
        kind: "feat",
        text: "WebGL-Lichter in Camera-Space wie SDF-Renderer",
        scope: "webgl-lights",
      },
    ],
  },
  {
    version: "0.6.288",
    entries: [
      {
        kind: "feat",
        text: "Drag aller Entity-Typen + Mirror-Hover/Pick/GPU-Race",
        scope: "sketch",
        tickets: ["matf", "o6hx"],
      },
    ],
  },
  {
    version: "0.6.287",
    entries: [
      {
        kind: "feat",
        text: "Coincident-Tool typabhängig — Punkt/Kurve/Linie/Kreis",
        scope: "sketch",
        tickets: ["uf62"],
      },
    ],
  },
  {
    version: "0.6.286",
    entries: [
      {
        kind: "feat",
        text: "Linien-Endpunkte snappen an Kreise/Bögen",
        scope: "sketch",
        tickets: ["fkig"],
      },
    ],
  },
  {
    version: "0.6.285",
    entries: [
      {
        kind: "fix",
        text: "Trim löscht keinen ganzen Kreis mehr bei <2 Schnittpunkten",
        scope: "sketch",
        tickets: ["rxi2"],
      },
    ],
  },
  {
    version: "0.6.284",
    entries: [
      {
        kind: "fix",
        text: "Variablenname in Zahlenfeld tippen schließt Detail nicht mehr",
        scope: "detail",
        tickets: ["6sjq"],
      },
    ],
  },
  {
    version: "0.6.283",
    entries: [
      {
        kind: "fix",
        text: "Variables-Panel auch im Sketch sichtbar — rechte Sidebar identisch",
        scope: "sidebar",
        tickets: ["v1a7"],
      },
    ],
  },
  {
    version: "0.6.282",
    entries: [
      {
        kind: "feat",
        text: "Vertices + Flächen messbar, alle Distanz-Kombinationen",
        scope: "measure",
        tickets: ["me9w"],
      },
    ],
  },
  {
    version: "0.6.281",
    entries: [
      {
        kind: "fix",
        text: "Profil-Query schließt eigene/spätere Output-Körper aus",
        scope: "query",
        tickets: ["2ukj"],
      },
    ],
  },
  {
    version: "0.6.280",
    entries: [
      {
        kind: "fix",
        text: "bogen-berandete Regionen (Linie+Arc) als Profil pickbar",
        scope: "sketch",
        tickets: ["6b41"],
      },
    ],
  },
  {
    version: "0.6.279",
    entries: [
      {
        kind: "fix",
        text: "Profil-Query je Region statt ganzem Sketch + Forward-Ref-Filter",
        scope: "extrude",
        tickets: ["tgdg"],
      },
    ],
  },
  {
    version: "0.6.278",
    entries: [
      {
        kind: "fix",
        text: "Szene-Klick kontextabhängig routen, Sketch bleibt offen",
        scope: "sketch",
        tickets: ["4anf"],
      },
    ],
  },
  {
    version: "0.6.277",
    entries: [
      {
        kind: "fix",
        text: "keine Node-Materialien mehr im klassischen Renderer",
        scope: "webgl",
        tickets: ["n6rs", "gsoj"],
      },
    ],
  },
  {
    version: "0.6.276",
    entries: [
      {
        kind: "feat",
        text: "assoziative B-Rep-Kanten-Projektion in den Sketch",
        scope: "sketch-project",
        tickets: ["jcb7"],
      },
    ],
  },
  {
    version: "0.6.275",
    entries: [
      {
        kind: "fix",
        text: "unvollständiger Revolve ist invalid + Warnung statt Garbage-Körper",
        scope: "revolve",
        tickets: ["ni56"],
      },
    ],
  },
  {
    version: "0.6.274",
    entries: [
      {
        kind: "feat",
        text: "ein smartes Wert-Textfeld pro Variable — Einheit/Formel implizit, kein ƒx/Typ-Selektor",
        scope: "variables",
        tickets: ["buy9", "nlij"],
      },
    ],
  },
  {
    version: "0.6.273",
    entries: [
      {
        kind: "feat",
        text: 'Pick-to-apply statt „Enter wendet an"',
        scope: "sketch-mirror",
        tickets: ["ps21"],
      },
    ],
  },
  {
    version: "0.6.272",
    entries: [
      {
        kind: "fix",
        text: "direkter Maus-Down+Drag greift Sketch-Punkt sofort",
        scope: "sketch",
        tickets: ["t11x"],
      },
    ],
  },
  {
    version: "0.6.271",
    entries: [
      {
        kind: "fix",
        text: "ViewCube-Klicks & View-Commands im WebGL-Modus",
        scope: "webgl-viewcube",
        tickets: ["10cl"],
      },
    ],
  },
  {
    version: "0.6.270",
    entries: [
      {
        kind: "fix",
        text: "Move-Gizmo im WebGL-Renderer sichtbar + interaktiv",
        scope: "webgl-gizmo",
        tickets: ["7hk3"],
      },
    ],
  },
  {
    version: "0.6.269",
    entries: [
      {
        kind: "feat",
        text: "Revolve-Profil per geschlossener Sketch-Region statt ganzem Sketch",
        scope: "revolve",
        tickets: ["dhjn"],
      },
    ],
  },
  {
    version: "0.6.268",
    entries: [
      {
        kind: "feat",
        text: "allgemeines Picking/Selektion/Hover/Measure im WebGL-Modus",
        scope: "webgl",
        tickets: ["814w"],
      },
    ],
  },
  {
    version: "0.6.264",
    entries: [
      {
        kind: "fix",
        text: "vlhp-Speichen — opake Proxy-Position per Gradient-Newton statt View-Ray polieren",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.263",
    entries: [
      {
        kind: "fix",
        text: "Revolve-Unterseite-Speichen — opake Proxy-Position auf echten SDF polieren",
        scope: "sdf-deferred",
        tickets: ["vlhp"],
      },
    ],
  },
  {
    version: "0.6.262",
    entries: [
      {
        kind: "fix",
        text: "Vertex-Dots beim Endpunkt-Drag nicht mehr neu erzeugen",
        scope: "sketch",
        tickets: ["vwzj"],
      },
    ],
  },
  {
    version: "0.6.261",
    entries: [
      {
        kind: "feat",
        text: "entity-basiertes Measure-Tool mit schwebendem Panel",
        scope: "measure",
        tickets: ["stv8"],
      },
    ],
  },
  {
    version: "0.6.260",
    entries: [
      {
        kind: "fix",
        text: "destruktiven Rebuild bis GPU-Idle deferren",
        scope: "sdf-deferred",
        tickets: ["sgzw"],
      },
    ],
  },
  {
    version: "0.6.259",
    entries: [
      {
        kind: "feat",
        text: "Move-Modus — Klick auf Körper legt/reused Translate-Feature",
        scope: "transform",
      },
    ],
  },
  {
    version: "0.6.258",
    entries: [
      {
        kind: "feat",
        text: "schwebende Tool-Hint + Mirror-axis-first + Move-click-drag",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.257",
    entries: [
      {
        kind: "fix",
        text: "ViewCube/View-Commands — Hit-Testing + Consumer ohne Controls",
        scope: "view",
        tickets: ["xw51"],
      },
    ],
  },
  {
    version: "0.6.256",
    entries: [
      {
        kind: "fix",
        text: "Rechteck-/Polygon-Kanten als Revolve-Achse wählbar",
        scope: "sketch",
        tickets: ["pr8a"],
      },
    ],
  },
  {
    version: "0.6.255",
    entries: [
      {
        kind: "fix",
        text: "3-Punkt-Kreis behält die 3 Punkte als verschiebbare Entities",
        scope: "sketch",
        tickets: ["ybnl"],
      },
    ],
  },
  {
    version: "0.6.254",
    entries: [
      {
        kind: "fix",
        text: "H/V/Parallel-Snap winkelbasiert statt distanzbasiert",
        scope: "sketch",
        tickets: ["4peq"],
      },
    ],
  },
  {
    version: "0.6.253",
    entries: [
      {
        kind: "fix",
        text: "Snap auf eine Linie erzeugt ein persistentes Constraint",
        scope: "sketch",
        tickets: ["inej"],
      },
    ],
  },
  {
    version: "0.6.252",
    entries: [
      {
        kind: "feat",
        text: "neuer Sketch erscheint sofort als Feature",
        scope: "sketch",
        tickets: ["d9o9"],
      },
    ],
  },
  {
    version: "0.6.251",
    entries: [
      {
        kind: "fix",
        text: 'Context-Menu „Confirm Sketch" schließt den Detail-Dialog',
        scope: "sketch",
        tickets: ["rkez"],
      },
    ],
  },
  {
    version: "0.6.250",
    entries: [
      {
        kind: "feat",
        text: "Typ (Number/Length/Angle) + Einheiten",
        scope: "variables",
        tickets: ["8ar9", "eil"],
      },
    ],
  },
  {
    version: "0.6.249",
    entries: [
      {
        kind: "feat",
        text: "Variablen/Formeln in number- und vec-Feldern nutzbar",
        scope: "params",
        tickets: ["x2m7"],
      },
    ],
  },
  {
    version: "0.6.248",
    entries: [
      {
        kind: "feat",
        text: "Variablen umbenennbar",
        scope: "variables",
        tickets: ["8ar9", "eil"],
      },
    ],
  },
  {
    version: "0.6.247",
    entries: [
      {
        kind: "fix",
        text: "Bodies-Query filtert auf BODY + Detail-Header zeigt prettified Namen",
        scope: "material/outliner",
        tickets: ["o9ul"],
      },
    ],
  },
  {
    version: "0.6.246",
    entries: [
      {
        kind: "feat",
        text: "Report-Dialog nicht-modal, verschiebbar und minimierbar",
        scope: "report",
        tickets: ["ttzl"],
      },
    ],
  },
  {
    version: "0.6.245",
    entries: [
      {
        kind: "fix",
        text: "bodyHybridInfo darf bei Out-of-Range-Index nicht panicken",
        scope: "kernel",
        tickets: ["c990"],
      },
    ],
  },
  {
    version: "0.6.243",
    entries: [
      {
        kind: "fix",
        text: "synchronen GPU-Dispose im Daten-Update deferren",
        scope: "sdf-deferred",
        tickets: ["sgzw"],
      },
    ],
  },
  {
    version: "0.6.242",
    entries: [
      {
        kind: "feat",
        text: "F2 öffnet den Report-Issue-Dialog",
        scope: "shortcuts",
      },
    ],
  },
  {
    version: "0.6.240",
    entries: [
      {
        kind: "fix",
        text: "Revolve-Proxy-Mesh schließen — Loch verursachte deferred Inside-out",
        scope: "sdf-deferred",
        tickets: ["ceat", "0b8m", "a72n"],
      },
    ],
  },
  {
    version: "0.6.239",
    entries: [
      {
        kind: "fix",
        text: "Auto-Reload-Off greift jetzt auch bei Server-Neustart",
        scope: "dev",
        tickets: ["4kky"],
      },
    ],
  },
  {
    version: "0.6.238",
    entries: [
      {
        kind: "feat",
        text: "FreeCAD-Preset ans Wiki angeglichen + SolidWorks ergänzt",
        scope: "controls",
        tickets: ["8ikq", "eil"],
      },
    ],
  },
  {
    version: "0.6.237",
    entries: [
      {
        kind: "fix",
        text: 'expliziter Per-Frame-Canvas-Clear gegen "Nachziehen"',
        scope: "sdf-deferred",
        tickets: ["ceat", "awq6"],
      },
    ],
  },
  {
    version: "0.6.236",
    entries: [
      {
        kind: "feat",
        text: "Blender-Preset ergänzt",
        scope: "controls",
        tickets: ["8ikq", "eil"],
      },
    ],
  },
  {
    version: "0.6.235",
    entries: [
      {
        kind: "feat",
        text: "Three.js-Preset — Shift+LMB-Drag = Box-Select",
        scope: "controls",
        tickets: ["8ikq", "eil"],
      },
    ],
  },
  {
    version: "0.6.234",
    entries: [
      {
        kind: "feat",
        text: "Onshape als Default-Preset, FieldDraft-Preset entfernt",
        scope: "controls",
        tickets: ["8ikq", "eil"],
      },
    ],
  },
  {
    version: "0.6.233",
    entries: [
      {
        kind: "feat",
        text: "Toggle für Auto-Reload bei Codeänderung",
        scope: "settings",
        tickets: ["cztw"],
      },
    ],
  },
  {
    version: "0.6.232",
    entries: [
      {
        kind: "fix",
        text: "Welt-Gitter nicht mehr von der near-Plane abgeschnitten",
        scope: "camera",
        tickets: ["xv9c"],
      },
    ],
  },
  {
    version: "0.6.231",
    entries: [
      {
        kind: "fix",
        text: "View Normal to Sketch Plane respektiert Kamera-UP",
        scope: "camera",
        tickets: ["s8l0"],
      },
    ],
  },
  {
    version: "0.6.230",
    entries: [
      {
        kind: "fix",
        text: "Rechts-Drag öffnet kein Context Menu mehr — nur Rechts-Klick",
        scope: "viewport",
        tickets: ["qi81"],
      },
    ],
  },
  {
    version: "0.6.229",
    entries: [
      {
        kind: "fix",
        text: "Außen-Orientierung für alle Extrusionsarten generalisieren — Sweep/Helix/Revolve",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.228",
    entries: [
      {
        kind: "fix",
        text: "Revolve-Mesh inside-out — Lathe-Winding+Normalen generisch nach außen zwingen",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.227",
    entries: [
      {
        kind: "perf",
        text: "Revolve/full-SDF-Körper marschieren echtes Leaf-Mesh statt AABB-Box",
        scope: "sdf-deferred",
      },
    ],
  },
  {
    version: "0.6.226",
    entries: [
      {
        kind: "fix",
        text: 'Pipeline-Teardown deferred disposen — "Buffer used while destroyed"',
        scope: "sdf",
        tickets: ["323i"],
      },
    ],
  },
  {
    version: "0.6.225",
    entries: [
      {
        kind: "fix",
        text: "Grid + Datum-Planes fehlten — auf OVERLAY_LAYER ziehen",
        scope: "sdf-deferred",
        tickets: ["p8dp"],
      },
    ],
  },
  {
    version: "0.6.224",
    entries: [
      {
        kind: "feat",
        text: "Hover-Highlight für Sketch-Entities im Select-Modus",
        scope: "sketch",
        tickets: ["zadt"],
      },
    ],
  },
  {
    version: "0.6.223",
    entries: [
      {
        kind: "fix",
        text: "Sketch-Detail-Header zeigt editierbaren Namen + ✓/✗ schließen mit einem Klick",
        scope: "outliner",
      },
    ],
  },
  {
    version: "0.6.222",
    entries: [
      {
        kind: "fix",
        text: "drei identische ☰ entzerren — Panel-Chevron rechts, Toolbar zeigt aktives Tool",
        scope: "mobile",
        tickets: ["qh4l"],
      },
    ],
  },
  {
    version: "0.6.221",
    entries: [
      {
        kind: "feat",
        text: "3D-Rechtsklick-Kontextmenü + editierbare Bemaßung mit Einheiten",
        scope: "viewport,sketch",
      },
    ],
  },
  {
    version: "0.6.220",
    entries: [
      {
        kind: "feat",
        text: "Toolbar wird auf Mobile-Fenstergröße zu einem Menü",
        scope: "toolbar",
        tickets: ["apr3"],
      },
    ],
  },
  {
    version: "0.6.219",
    entries: [
      {
        kind: "fix",
        text: "achsen-ausgerichteter Revolve war ein LEERER SDF-Solid — Profil auf +x spiegeln",
        scope: "kernel",
        tickets: ["a204", "ifn2"],
      },
    ],
  },
  {
    version: "0.6.218",
    entries: [
      {
        kind: "fix",
        text: "Revolve-Mesh bei achsen-ausgerichtetem Profil mit negativem x korrekt rendern",
        scope: "render",
        tickets: ["ngj1", "a204", "ifn2"],
      },
    ],
  },
  {
    version: "0.6.217",
    entries: [
      {
        kind: "fix",
        text: "Revolve-Achse als Sketch-Linie im WebGL-Renderer pickbar",
        scope: "webgl",
        tickets: ["6vdf"],
      },
    ],
  },
  {
    version: "0.6.216",
    entries: [
      {
        kind: "fix",
        text: "Achsen-Pick-Freeze im Default-Renderer (sdf-deferred) wirklich beheben — GPU-Disposal an onSubmittedWorkDone koppeln",
        scope: "render",
        tickets: ["fg9o"],
      },
    ],
  },
  {
    version: "0.6.215",
    entries: [
      {
        kind: "fix",
        text: "GPU-Ressourcen verzögert freigeben — kein Einfrieren nach Achsen-Pick",
        scope: "sdf-vm",
        tickets: ["fg9o"],
      },
    ],
  },
  {
    version: "0.6.214",
    entries: [
      {
        kind: "feat",
        text: "Dimension-Tool zeigt Maßlinien + Linien-Hover",
        scope: "sketch",
        tickets: ["7lxk"],
      },
    ],
  },
  {
    version: "0.6.213",
    entries: [
      {
        kind: "fix",
        text: "Snap auf vorhandene Sketch-Linien (Punkt-Tool + Linien-Endpunkt)",
        scope: "sketch",
        tickets: ["pgyf"],
      },
    ],
  },
  {
    version: "0.6.212",
    entries: [
      {
        kind: "fix",
        text: "Spline-Preview folgt dem Cursor während des Drags",
        scope: "sketch",
        tickets: ["8vpe"],
      },
    ],
  },
  {
    version: "0.6.211",
    entries: [
      {
        kind: "feat",
        text: "Line-Tool als verkettetes Rubber-Band (Polylinie)",
        scope: "sketch",
        tickets: ["daa8"],
      },
    ],
  },
  {
    version: "0.6.210",
    entries: [
      {
        kind: "fix",
        text: "Mirror-Tool — Achsen-Linie per Klick wählbar + Hover-Highlight + Hinweis",
        scope: "sketch",
        tickets: ["czqf"],
      },
    ],
  },
  {
    version: "0.6.209",
    entries: [
      {
        kind: "feat",
        text: "Fillet/Chamfer per Einzelklick auf Eckpunkt — auch bei Polygon/N-Gon/Rechteck",
        scope: "sketch",
        tickets: ["tqi4"],
      },
    ],
  },
  {
    version: "0.6.208",
    entries: [
      {
        kind: "fix",
        text: "Spline-Tool verlassen FINISHT die Spline statt sie zu verwerfen",
        scope: "sketch",
        tickets: ["egia"],
      },
    ],
  },
  {
    version: "0.6.207",
    entries: [
      {
        kind: "fix",
        text: "N-Gon-Outline rendern — verschwindet nicht mehr beim Loslassen",
        scope: "sketch",
        tickets: ["8vb5"],
      },
    ],
  },
  {
    version: "0.6.206",
    entries: [
      {
        kind: "feat",
        text: "Action-Log loggt jetzt ALLE Zeichen + Feldwerte (Dev-Phase, kein Datenschutz-Filter)",
        scope: "report",
      },
    ],
  },
  {
    version: "0.6.205",
    entries: [
      {
        kind: "fix",
        text: "keine BRep-Kanten/Flächen-Hover im Sketch-Modus",
        scope: "sketch",
        tickets: ["94jk"],
      },
    ],
  },
  {
    version: "0.6.204",
    entries: [
      {
        kind: "feat",
        text: 'User-Aktions-Log fürs "Report Issue" — replaybare Schritt-Spur',
        scope: "report",
      },
    ],
  },
  {
    version: "0.6.203",
    entries: [
      {
        kind: "fix",
        text: 'Single-Query-Picker nach Pick freigeben (one-shot) — kein "stuck"-Zustand mehr',
        scope: "query",
        tickets: ["e9il"],
      },
    ],
  },
  {
    version: "0.6.202",
    entries: [
      {
        kind: "fix",
        text: "Szenen-Hintergrund wieder weiß — ACES-Tonemapping deaktiviert",
        scope: "render",
        tickets: ["7kht"],
      },
    ],
  },
  {
    version: "0.6.201",
    entries: [
      {
        kind: "fix",
        text: "Klick im Szene-View bei fokussiertem Query-Feld selektiert statt Detail zu schließen",
        scope: "query",
        tickets: ["fs23"],
      },
    ],
  },
  {
    version: "0.6.200",
    entries: [
      {
        kind: "fix",
        text: "redundante Feature-Titelzeile aus dem Detail-View entfernen",
        scope: "ui",
        tickets: ["gqjt"],
      },
    ],
  },
  {
    version: "0.6.199",
    entries: [
      {
        kind: "feat",
        text: "Constraints als Tools mit Labels in der Tool-Zeile, tool-first Workflow",
        scope: "sketch",
        tickets: ["zz1r"],
      },
    ],
  },
  {
    version: "0.6.198",
    entries: [
      {
        kind: "feat",
        text: 'Detail-Header zeigt editierbaren Feature-Namen statt "Detail"',
        scope: "ui",
        tickets: ["f7xk"],
      },
    ],
  },
  {
    version: "0.6.197",
    entries: [
      {
        kind: "fix",
        text: "BRep-Kanten im WebGL-Renderer darstellen",
        scope: "webgl",
        tickets: ["jnej"],
      },
    ],
  },
  {
    version: "0.6.196",
    entries: [
      {
        kind: "fix",
        text: "committeter sichtbarer Sketch in Grau statt Blau",
        scope: "sketch",
        tickets: ["imqf"],
      },
    ],
  },
  {
    version: "0.6.195",
    entries: [
      {
        kind: "feat",
        text: "Warnung wenn die Achse nicht in der Sketch-Ebene liegt",
        scope: "revolve/helix",
        tickets: ["dnfe"],
      },
    ],
  },
  {
    version: "0.6.194",
    entries: [
      {
        kind: "feat",
        text: "Helix-End-Flächen selektierbar — Deckel als analytische Plane-Faces + Provenance",
        scope: "brep",
        tickets: ["exus"],
      },
    ],
  },
  {
    version: "0.6.192",
    entries: [
      {
        kind: "fix",
        text: "Helix/Sweep-Längskanten als glatte NURBS statt Polyline",
        scope: "brep",
        tickets: ["1t40"],
      },
    ],
  },
  {
    version: "0.6.191",
    entries: [
      {
        kind: "fix",
        text: "adaptive Near/Far-Clip — Körper verschwinden beim Wegzoomen nicht mehr",
        scope: "camera",
        tickets: ["lbzq"],
      },
    ],
  },
  {
    version: "0.6.190",
    entries: [
      {
        kind: "feat",
        text: "konstruktive Helix- & Sweep-Kanten (Profil-Ecken durch die Bewegung tracen)",
        scope: "brep",
        tickets: ["ifog", "eil"],
      },
    ],
  },
  {
    version: "0.6.189",
    entries: [
      {
        kind: "feat",
        text: "konstruktive Revolve-Kanten — Profil-Ecken zu Kreisen tracen",
        scope: "brep",
        tickets: ["ifog", "eil"],
      },
    ],
  },
  {
    version: "0.6.188",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Seitenkanten bei schrägen Profilwänden (line_point-Determinante)",
        scope: "brep",
        tickets: ["ifog"],
      },
    ],
  },
  {
    version: "0.6.187",
    entries: [
      {
        kind: "feat",
        text: "UI-State als ui_state.json im Ticket mitspeichern",
        scope: "report",
        tickets: ["uz5q"],
      },
    ],
  },
  {
    version: "0.6.186",
    entries: [
      {
        kind: "fix",
        text: "committeter Sketch zeichnet auch Splines — schließt Spline-Loop",
        scope: "sketch",
        tickets: ["sc8g"],
      },
    ],
  },
  {
    version: "0.6.185",
    entries: [
      {
        kind: "fix",
        text: "entity_filter erreicht die Picker wieder (snake_case + multi_query)",
        scope: "schema",
      },
    ],
  },
  {
    version: "0.6.184",
    entries: [
      {
        kind: "fix",
        text: "Query-Felder akzeptieren nur Picks ihres entity_filter",
        scope: "picker",
      },
    ],
  },
  {
    version: "0.6.183",
    entries: [
      {
        kind: "feat",
        text: "Achsen-Query (Sketch-Linie ODER BRep-Kante) wie bei Revolve",
        scope: "helix",
      },
    ],
  },
  {
    version: "0.6.182",
    entries: [
      {
        kind: "fix",
        text: "sichtbarer Sketch zeigt auch Linien+Fill, nicht nur Eckpunkte",
        scope: "webgl",
        tickets: ["fdzj"],
      },
    ],
  },
  {
    version: "0.6.181",
    entries: [
      {
        kind: "feat",
        text: "Achsen-Query (Sketch-Linie ODER BRep-Kante) statt fixer Y-Achse",
        scope: "revolve",
        tickets: ["b8qh"],
      },
    ],
  },
  {
    version: "0.6.180",
    entries: [
      {
        kind: "fix",
        text: "Maus-Preset auch im WebGL-Viewport (CameraRig statt blanker OrbitControls)",
        scope: "webgl",
        tickets: ["owia"],
      },
    ],
  },
  {
    version: "0.6.179",
    entries: [
      {
        kind: "fix",
        text: "Sketch-Live-Preview, Sketch-Undo, parametrische Helix",
        scope: "webgl",
        tickets: ["x941"],
      },
    ],
  },
  {
    version: "0.6.178",
    entries: [
      {
        kind: "fix",
        text: "Ticket-Settings als settings.json + project.fdraft als echtes ZIP",
        scope: "report",
        tickets: ["478k"],
      },
    ],
  },
  {
    version: "0.6.177",
    entries: [
      {
        kind: "fix",
        text: "Sketch-Linien sichtbar + Plane-Pick aus Szene im WebGL-Modus",
        scope: "webgl",
        tickets: ["uzhr"],
      },
    ],
  },
  {
    version: "0.6.176",
    entries: [
      {
        kind: "fix",
        text: "SDF-Deferred zeigt Geometrie wieder + weißer BG korrekt",
        scope: "render",
        tickets: ["qz52"],
      },
    ],
  },
  {
    version: "0.6.175",
    entries: [
      {
        kind: "feat",
        text: ".fdraft als ZIP-Container mit Asset-Persistenz",
        scope: "project",
        tickets: ["wu2v"],
      },
    ],
  },
  {
    version: "0.6.174",
    entries: [
      {
        kind: "feat",
        text: "Constraint-Toolbar Onshape-Style mit Icons + unified Dimension",
        scope: "sketch",
        tickets: ["vk8q"],
      },
    ],
  },
  {
    version: "0.6.173",
    entries: [
      {
        kind: "feat",
        text: "Sketch-Toolbar einzeilig — Import als Tool, DoF rechts",
        scope: "sketch",
        tickets: ["yuaw"],
      },
    ],
  },
  {
    version: "0.6.172",
    entries: [
      {
        kind: "fix",
        text: "Eckpunkt-Dots sichtbar — Mesh-Kreise statt nicht-rendernder Points",
        scope: "sketch",
        tickets: ["tdq2"],
      },
    ],
  },
  {
    version: "0.6.171",
    entries: [
      {
        kind: "fix",
        text: "SDF-Szenen-Hintergrund weiß statt grau",
        scope: "render",
        tickets: ["a2ps"],
      },
    ],
  },
  {
    version: "0.6.170",
    entries: [
      {
        kind: "feat",
        text: "Per-Device-Settings als JSON ins Ticket aufnehmen",
        scope: "report",
      },
    ],
  },
  {
    version: "0.6.169",
    entries: [
      {
        kind: "feat",
        text: "Session-Lifecycle — sticky gegen Szene-Klicks, Extrude/Revolve in Toolbar, Delete/New-Project-Reset",
        scope: "sketch",
        tickets: ["7hgr"],
      },
    ],
  },
  {
    version: "0.6.168",
    entries: [
      {
        kind: "fix",
        text: "leeres Text-Input löscht den Text im Edit-Modus",
        scope: "sketch",
        tickets: ["g91j"],
      },
    ],
  },
  {
    version: "0.6.167",
    entries: [
      {
        kind: "fix",
        text: "Plane-Feld wie generisches Query-Input + Plane-Pick aus Szene",
        scope: "sketch",
        tickets: ["3btx"],
      },
    ],
  },
  {
    version: "0.6.166",
    entries: [
      {
        kind: "feat",
        text: "weißer Szenen-Hintergrund + runde Eckpunkt-Dots in doppelter Linienbreite",
        scope: "sketch",
        tickets: ["xqp3"],
      },
    ],
  },
  {
    version: "0.6.165",
    entries: [
      {
        kind: "feat",
        text: "Report-Issue ohne Title, kein ticket.json mehr",
        scope: "report",
        tickets: ["xqll"],
      },
    ],
  },
  {
    version: "0.6.164",
    entries: [
      {
        kind: "feat",
        text: "Sketch-Details als generischer Detail-Dialog",
        scope: "sketch",
        tickets: ["87a1"],
      },
    ],
  },
  {
    version: "0.6.163",
    entries: [
      {
        kind: "feat",
        text: "Eckpunkte/Vertices als Punkte visualisieren",
        scope: "sketch",
        tickets: ["n6hg"],
      },
    ],
  },
  {
    version: "0.6.162",
    entries: [
      {
        kind: "fix",
        text: "geladene Sketch-Objekte beim Editieren selektier/löschbar",
        scope: "sketch",
        tickets: ["o2y3"],
      },
    ],
  },
  {
    version: "0.6.161",
    entries: [
      {
        kind: "fix",
        text: "Text-Tool Feinschliff — leeres Feld pro Klick, kein Leer-Objekt, Tooltip",
        scope: "sketch",
        tickets: ["lycy"],
      },
    ],
  },
  {
    version: "0.6.160",
    entries: [
      {
        kind: "feat",
        text: "committeten Text in-scene re-editierbar",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.159",
    entries: [
      {
        kind: "feat",
        text: "Fillet-Radius/Chamfer-Distanz post-hoc in-scene editierbar",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.158",
    entries: [
      {
        kind: "feat",
        text: "n-gon/Pattern/Rotate-Parameter in-scene",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.157",
    entries: [
      {
        kind: "feat",
        text: "Booleans im WebGL-Renderer via DC-Mesh + Material-Parität",
        scope: "webgl",
        tickets: ["gcvz"],
      },
    ],
  },
  {
    version: "0.6.156",
    entries: [
      {
        kind: "feat",
        text: "Fillet/Chamfer/Offset-Eingabe in-scene + Plan-Doc für restliche Felder",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.155",
    entries: [
      {
        kind: "feat",
        text: "Text-Eingabe + Parameter in-scene statt Toolbar",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.154",
    entries: [
      {
        kind: "feat",
        text: "Gizmos ausblenden während aktivem Query-Pick",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.153",
    entries: [
      {
        kind: "feat",
        text: "explizite Constraint-Buttons + Query-Pick akzeptiert Sketch-Feature aus Liste",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.152",
    entries: [
      {
        kind: "feat",
        text: "Geometry-Accordion (collapsed) + Tool-Dropdowns für Circle/Rect-Gruppen",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.151",
    entries: [
      {
        kind: "feat",
        text: "Sketch ist ein Feature wie jedes andere — Selektion = Edit, ✓/✗ im Detail-Panel",
        scope: "sketch",
        tickets: ["pqoe", "eil"],
      },
    ],
  },
  {
    version: "0.6.150",
    entries: [
      {
        kind: "fix",
        text: "Klick auf committeten Sketch selektiert den Sketch, nicht die Datum-Plane dahinter",
        scope: "pick",
        tickets: ["md1e"],
      },
    ],
  },
  {
    version: "0.6.149",
    entries: [
      {
        kind: "feat",
        text: "committете Sketch-Darstellung exakt wie Sketch-Modus — stärkere Füllung + doppelte Stroke-Breite",
        scope: "sketch",
        tickets: ["jjmv"],
      },
    ],
  },
  {
    version: "0.6.148",
    entries: [
      {
        kind: "feat",
        text: "per-Body raymarch-Step-Relax-Mechanismus",
        scope: "sdf",
        tickets: ["1wnm"],
      },
    ],
  },
  {
    version: "0.6.147",
    entries: [
      {
        kind: "feat",
        text: "committете Sketch-Regionen gefüllt + als Profil pickbar",
        scope: "sketch",
        tickets: ["jjmv"],
      },
    ],
  },
  {
    version: "0.6.146",
    entries: [
      {
        kind: "feat",
        text: 'erster/freistehender Add ohne Scope = implizit "New", keine Warnung',
        scope: "snapshot",
      },
    ],
  },
  {
    version: "0.6.145",
    entries: [
      {
        kind: "fix",
        text: "Part-Farben + Default-Planes + ViewCube-Sync im WebGL-Renderer",
        scope: "webgl",
        tickets: ["59c2", "r8b4"],
      },
    ],
  },
  {
    version: "0.6.144",
    entries: [
      {
        kind: "feat",
        text: "Scope ist Pflicht für Remove/Intersect/Hole — kein impliziter AABB-Effekt, nur Warnung",
        scope: "snapshot",
      },
    ],
  },
  {
    version: "0.6.143",
    entries: [
      {
        kind: "feat",
        text: "Hover-Dimming + Pick-Reject-Feedback für späteres-Feature-Geometrie",
        scope: "snapshot",
      },
    ],
  },
  {
    version: "0.6.142",
    entries: [
      {
        kind: "feat",
        text: "expliziter Merge-Scope schlägt AABB — Subtract/Intersect treffen nur die gewählten Zielkörper",
        scope: "boolean",
      },
    ],
  },
  {
    version: "0.6.141",
    entries: [
      {
        kind: "feat",
        text: "UI-Auswahlfilter + Reorder-Guard — Features referenzieren nur frühere Features",
        scope: "snapshot",
      },
    ],
  },
  {
    version: "0.6.140",
    entries: [
      {
        kind: "fix",
        text: "Forward-Reference-Tools/Targets → klarer Feature-Error statt stillem No-op",
        scope: "boolean",
        tickets: ["9wxs"],
      },
    ],
  },
  {
    version: "0.6.139",
    entries: [
      {
        kind: "feat",
        text: "Merge-Scope-Warnung — Add/Remove/Intersect ohne gültigen Scope rot markiert",
        scope: "features",
      },
    ],
  },
  {
    version: "0.6.138",
    entries: [
      {
        kind: "fix",
        text: "berührende Solids verschmelzen nicht mehr automatisch — Merge nur per explizitem Link",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.137",
    entries: [
      {
        kind: "feat",
        text: "volles interaktives Drag-Handle-3D-Gizmo für Gizmo3d-Sweep-Pfade",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.136",
    entries: [
      {
        kind: "feat",
        text: 'Gizmo3d-Pfad per Toolbar — "Sweep 3D" Dropdown',
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.135",
    entries: [
      {
        kind: "feat",
        text: "Orthogonal-Sketches-Provider — Zwei-Ansichten-Raumkurve",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.134",
    entries: [
      {
        kind: "feat",
        text: "EdgeChain-Pfad-Provider — Sweep entlang Modell-Kanten",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.133",
    entries: [
      {
        kind: "feat",
        text: "offene Pfad-Skizzen — Sweep entlang A→B-Kurve",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.132",
    entries: [
      {
        kind: "feat",
        text: "Twist — Profil rotiert entlang des Spines",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.131",
    entries: [
      {
        kind: "feat",
        text: "GPU-VM-Opcodes — Sweep im sdf-deferred/vm sichtbar",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.130",
    entries: [
      {
        kind: "feat",
        text: "Toolbar-Button + add_sweep-API — Sweeps per UI erzeugbar",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.129",
    entries: [
      {
        kind: "feat",
        text: "Sketch2d-Pfad-Provider + getaggte PathSourceDef",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.128",
    entries: [
      {
        kind: "feat",
        text: "SweepFeature auf gekrümmten Pfad via Gizmo3d-Provider",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.127",
    entries: [
      {
        kind: "feat",
        text: "Gatekeeper-gegateter parametrischer Sweep-Leaf",
        scope: "sweep",
      },
    ],
  },
  {
    version: "0.6.126",
    entries: [
      {
        kind: "feat",
        text: "CsgNode::Sweep — gekrümmter Sweep als echtes Kernel-Primitiv",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.125",
    entries: [
      {
        kind: "feat",
        text: "modulare PathProvider-Schicht für Pfad-Quellen",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.124",
    entries: [
      {
        kind: "feat",
        text: "curved-Sweep CPU-SDF — Nearest + Profil im RMF-Frame + Cap-Komposition",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.122",
    entries: [
      {
        kind: "feat",
        text: "RMF-Sweep-Mesher + geteilter Shell-Assembler",
        scope: "webgl",
      },
    ],
  },
  {
    version: "0.6.121",
    entries: [
      {
        kind: "feat",
        text: "Sweep-Gatekeeper — Capsule-BVH Pre-Flight für parametrische Sweeps",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.120",
    entries: [
      {
        kind: "feat",
        text: "Twist/Bend als Domain-Warp-Meshes — Vertex-Deformation statt AABB",
        scope: "webgl",
      },
    ],
  },
  {
    version: "0.6.119",
    entries: [
      {
        kind: "feat",
        text: "parametrische Helix-Meshes — Sweep-Profil statt AABB-Box",
        scope: "webgl",
      },
    ],
  },
  {
    version: "0.6.118",
    entries: [
      {
        kind: "fix",
        text: "kein manuelles Resizen mehr nötig; StrictMode teilw. verschoben",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.117",
    entries: [
      {
        kind: "feat",
        text: "funktionierender WebGL-Render-Pfad ohne WebGPU",
        scope: "render",
      },
    ],
  },
  {
    version: "0.6.116",
    entries: [
      {
        kind: "feat",
        text: "Hybrid-Renderer Phase 2 — getrimmte Zylinder-Faces als Raster-Band",
        scope: "render",
      },
    ],
  },
  {
    version: "0.6.115",
    entries: [
      {
        kind: "feat",
        text: "Hybrid-Renderer Phase 1 — planare B-Rep-Flächen als exakter Raster-Mesh",
        scope: "render",
      },
    ],
  },
  {
    version: "0.6.114",
    entries: [
      {
        kind: "feat",
        text: "GPU-Adapter + Steady-State-FPS-Telemetrie; Konzept adaptives Rendering",
        scope: "telemetry",
      },
    ],
  },
  {
    version: "0.6.113",
    entries: [
      {
        kind: "fix",
        text: "klare WebGPU-Fehlermeldung statt kaputtem WebGL-Fallback",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.112",
    entries: [
      {
        kind: "feat",
        text: "Maß-/Constraint-/Snap-Labels als 3D-Billboards statt 2D-Overlay-Text",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.111",
    entries: [
      {
        kind: "feat",
        text: "Drag-Previews, Hover, Snap-Ring, projizierte Kanten → 3D — Overlay zeichnet nur noch Text+Marquee",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.110",
    entries: [
      {
        kind: "feat",
        text: "Selektions-Highlight + Grips → 3D-Scene-Geometrie",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.109",
    entries: [
      {
        kind: "fix",
        text: "Overlay-HUD synchron zum Render-Frame zeichnen — kein Nachlaufen mehr von Snap/Hover/Grips/Selektion/Bemaßung",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.108",
    entries: [
      {
        kind: "feat",
        text: "alle committeten Sketch-Geometrien als echte 3D-Scene-Geometrie — Overlay strokt keine Geometrie mehr",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.107",
    entries: [
      {
        kind: "feat",
        text: "committed Kern-Geometrie als echte 3D-Scene-Geometrie statt 2D-Overlay-Reprojection",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.106",
    entries: [
      {
        kind: "feat",
        text: "Onshape-Parameter — Input-Modi, Height, Start-Winkel + Merge scope für Helix/Extrude/Revolve",
        scope: "helix",
        tickets: ["vfqh", "15f5"],
      },
    ],
  },
  {
    version: "0.6.105",
    entries: [
      {
        kind: "fix",
        text: "kontinuierlicher Sweep-Parameter-Clamp — kein degeneriertes Band am Helix-Ende",
        scope: "helix",
        tickets: ["vtyh"],
      },
    ],
  },
  {
    version: "0.6.104",
    entries: [
      {
        kind: "feat",
        text: "dezente Linien-Bemaßung mit Klick-zu-Edit statt mitfahrendem Eingabefeld",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.6.103",
    entries: [
      {
        kind: "perf",
        text: "Geschwister-Zellen teilen Corner-Evals über das 3×3×3-Gitter",
        scope: "octree",
      },
    ],
  },
  {
    version: "0.6.102",
    entries: [
      {
        kind: "feat",
        text: "Helix-Feature — Sketch-Profil helikal sweepen für echte Schraubgewinde",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.101",
    entries: [
      {
        kind: "feat",
        text: "Sketch-Extrudes als Pending-Ghost mit Sketch-Plane-Anker",
        scope: "extrude",
      },
    ],
  },
  {
    version: "0.6.100",
    entries: [
      {
        kind: "feat",
        text: "F0-1 — Face pro zusammenhängender Grenzregion statt pro Branch",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.99",
    entries: [
      {
        kind: "feat",
        text: "asynchroner Overlay-Weave — Weaver-Worker webt off-thread, Main-Kernel importiert",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.98",
    entries: [
      {
        kind: "feat",
        text: "Live-Extrude-Drag — Tiefe committet throttled während des Drags",
        scope: "extrude",
      },
    ],
  },
  {
    version: "0.6.97",
    entries: [
      {
        kind: "fix",
        text: "B-Rep-Overlays während Gizmo-Drag komplett ausblenden",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.96",
    entries: [
      {
        kind: "feat",
        text: "BRep-Weave-Unterdrückung während Gizmo-Drags + Worker-Dauerunterdrückung",
        scope: "perf",
      },
    ],
  },
  {
    version: "0.6.95",
    entries: [
      {
        kind: "feat",
        text: "nicht-koaxiale RevolvedProfile-Paare im 2D-Scan + Tracer-Fixes",
        scope: "ssi",
      },
    ],
  },
  {
    version: "0.6.94",
    entries: [
      {
        kind: "perf",
        text: "Lower-Bound-Pruning im Carrier-Closest-Point",
        scope: "protocol",
      },
    ],
  },
  {
    version: "0.6.93",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Kreiswand-Carrier um 90° auf die Extrusionsachse gedreht",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.92",
    entries: [
      {
        kind: "feat",
        text: "retained-boundary Open-Shell — konstante Wandstärke statt Messerkanten-Taper",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.91",
    entries: [
      {
        kind: "feat",
        text: "Shell-bewusster Seed-Weave — Kavitätskanten analytisch, Vase-Ringe exakt",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.90",
    entries: [
      {
        kind: "feat",
        text: "koaxialer SSI-Fall — Revolve-Kanten als exakte Kreise aus 2D-Profilschnitten",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.89",
    entries: [
      {
        kind: "feat",
        text: "HalfSpace::RevolvedProfile — Spline-Revolve-Wände analytisch in der Branch-Map",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.88",
    entries: [
      {
        kind: "feat",
        text: "Hemisphären-AO im SDF-Shading — Kavitäten lesbar statt Deckel-Illusion",
        scope: "sdf-vm",
      },
    ],
  },
  {
    version: "0.6.87",
    entries: [
      {
        kind: "feat",
        text: "adaptives Spline-Lowering — exakte Cubic-Béziers statt 16-Schritt-Sampling",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.85",
    entries: [
      {
        kind: "feat",
        text: "OP_PRIM2D_PATH — analytische Pfad-Segmente statt 2308-Vertex-Flattening",
        scope: "sdf-vm",
      },
    ],
  },
  {
    version: "0.6.84",
    entries: [
      {
        kind: "fix",
        text: "POLY/SHELL-Loops im GPU-Interpreter dynamisch statt 32er-Cap",
        scope: "sdf-vm",
      },
    ],
  },
  {
    version: "0.6.82",
    entries: [
      {
        kind: "fix",
        text: "Projekt-Load fror den Tab ein — Weave 6× schneller, Boot ohne Sync-Mesh/Weave, Weave-Cache über Regens",
        scope: "perf",
      },
    ],
  },
  {
    version: "0.6.81",
    entries: [
      {
        kind: "fix",
        text: "Octree-Zellen-Overlay session-only — persistiertes 'true' fror den Tab beim Reload ein",
        scope: "settings",
      },
    ],
  },
  {
    version: "0.6.80",
    entries: [
      {
        kind: "feat",
        text: "Phase 4 — Octree-Zellklassifikation auf Koinzidenz-Klassen",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.79",
    entries: [
      {
        kind: "fix",
        text: "GPU-Path-Polygon schließt über die echte Lücke + Shell-Kind im Outliner",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.78",
    entries: [
      {
        kind: "fix",
        text: "Revolve strippt Achsen-Segmente — Shell hinterlässt keinen Phantom-Stab",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.77",
    entries: [
      {
        kind: "feat",
        text: "K7.1 Loop-Inferenz — freie Linien/offene Splines bilden Regionen",
        scope: "kernel",
      },
    ],
  },
  {
    version: "0.6.76",
    entries: [
      {
        kind: "feat",
        text: "Entf/Backspace löscht selektierte Sketch-Elemente",
        scope: "ux",
      },
    ],
  },
  {
    version: "0.6.75",
    entries: [
      {
        kind: "fix",
        text: "Datum-Planes als Sketch-Plane pickbar + Query-Picker entschärft",
        scope: "ux",
      },
    ],
  },
  {
    version: "0.6.68",
    entries: [
      {
        kind: "feat",
        text: "BRep-Basiskanten 2.5px, Hover 3px",
        scope: "viewport",
      },
    ],
  },
  {
    version: "0.6.67",
    entries: [
      {
        kind: "feat",
        text: "Onshape-artige Fehler-Toleranz im Regen-Loop + Extrude/Revolve ohne Selektion",
        scope: "features",
        tickets: ["sx5a", "1rax"],
      },
    ],
  },
  {
    version: "0.6.66",
    entries: [
      {
        kind: "fix",
        text: "Selektions-/Hover-Darstellung Face-präzise statt Branch-weit",
        scope: "viewport",
        tickets: ["b91c"],
      },
    ],
  },
  {
    version: "0.6.65",
    entries: [
      {
        kind: "feat",
        text: "Pending-Ghost bis zur Bestätigung + Negativ-Tiefe durch die Basisebene",
        scope: "extrude",
        tickets: ["x7af"],
      },
    ],
  },
  {
    version: "0.6.64",
    entries: [
      {
        kind: "fix",
        text: "Hover-Kanten 2.5px + helleres Orange, Face-Hover-Outline als Fat-Line auf WebGPU sichtbar",
        scope: "viewport",
        tickets: ["bqvz"],
      },
    ],
  },
  {
    version: "0.6.63",
    entries: [
      {
        kind: "fix",
        text: "Phase 3 voll — CoincidentSurface-Gate, On-Boundary-Aktivität, Interior-Fin-Beweis schließen Ticket o2j9",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.62",
    entries: [
      {
        kind: "fix",
        text: "Face-Extrude ankert exakt auf analytischer Ebene + robuste Koinzidenz-Kanonikalisierung",
        scope: "brep",
        tickets: ["o2j9", "eil"],
      },
    ],
  },
  {
    version: "0.6.61",
    entries: [
      {
        kind: "fix",
        text: "Phasen 2+3 Koinzidenz-Kanonikalisierung — o2j9-Familie webt mannigfaltig",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.60",
    entries: [
      {
        kind: "feat",
        text: "Phase 1 Koinzidenz-Registry — orientierte Äquivalenzklassen, surface-generisch",
        scope: "brep",
        tickets: ["o2j9"],
      },
    ],
  },
  {
    version: "0.6.59",
    entries: [
      {
        kind: "fix",
        text: "F0-2 Loop-Orientierung — Komplement-Parität + gesampelte Windung, Manifold-Konvention auf gekrümmten Flächen",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.55",
    entries: [
      {
        kind: "fix",
        text: "Face-Extrude dockt an Eltern-Body statt fremder AABB-Überlappung",
        scope: "brep",
        tickets: ["px2o"],
      },
    ],
  },
  {
    version: "0.6.54",
    entries: [
      {
        kind: "fix",
        text: "Sketch-Tools nach Plane-Pick funktionsfähig — One-Shot-Picker",
        scope: "web",
        tickets: ["dxrq"],
      },
    ],
  },
  {
    version: "0.6.53",
    entries: [
      {
        kind: "fix",
        text: "Sketch-Plane ohne Koordinatengitter",
        scope: "web",
        tickets: ["g5km"],
      },
    ],
  },
  {
    version: "0.6.52",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Gizmo zeigt entlang Extrude-Achse statt Welt-äußerster Fläche",
        scope: "web",
        tickets: ["zzl1"],
      },
    ],
  },
  {
    version: "0.6.51",
    entries: [
      {
        kind: "fix",
        text: "Kerben in fetten Highlight-Kanten durch kollineare Dezimierung",
        scope: "web",
        tickets: ["mcvp"],
      },
    ],
  },
  {
    version: "0.6.49",
    entries: [
      {
        kind: "feat",
        text: "Sketch direkt in der 3D-Szene — normale Kamera, Projektion durchs Plane-Frame",
        scope: "web",
        tickets: ["bxoo"],
      },
    ],
  },
  {
    version: "0.6.48",
    entries: [
      {
        kind: "feat",
        text: "Sketch-Overlay nutzt echte Plane-Frames",
        scope: "web",
        tickets: ["yx6a", "bxoo"],
      },
    ],
  },
  {
    version: "0.6.46",
    entries: [
      {
        kind: "feat",
        text: "Sketch startet sofort, Plane als editierbares Session-Property",
        scope: "web",
        tickets: ["yx6a"],
      },
    ],
  },
  {
    version: "0.6.45",
    entries: [
      {
        kind: "fix",
        text: "Depth-Bias gegen Z-Fighting der Kanten im visible-only-Modus",
        scope: "web",
        tickets: ["b9xz"],
      },
    ],
  },
  {
    version: "0.6.44",
    entries: [
      {
        kind: "fix",
        text: "selektierte Sphere-/Torus-Face tintet das Primitiv orange",
        scope: "web",
        tickets: ["yiis"],
      },
    ],
  },
  {
    version: "0.6.43",
    entries: [
      {
        kind: "feat",
        text: "sketchPlaneFrame — web-seitige Plane-Frame-Auflösung + Ray-Plane-Input-Mathe",
        scope: "web",
        tickets: ["yx6a", "bxoo", "hase"],
      },
    ],
  },
  {
    version: "0.6.42",
    entries: [
      {
        kind: "fix",
        text: "selektierte Kanten 4px statt 2px",
        scope: "web",
        tickets: ["6btt"],
      },
    ],
  },
  {
    version: "0.6.41",
    entries: [
      {
        kind: "feat",
        text: "Sketch-Plane im Detail-Panel editierbar",
        scope: "web",
        tickets: ["yx6a", "hase"],
      },
    ],
  },
  {
    version: "0.6.40",
    entries: [
      {
        kind: "fix",
        text: "editSketch round-trippt die Plane-Query + Sketch-in-3D-Plan",
        scope: "web",
        tickets: ["yx6a", "bxoo"],
      },
    ],
  },
  {
    version: "0.6.39",
    entries: [
      {
        kind: "fix",
        text: "Sketch-2D-Overlay vertikal an die 3D-Kamera angeglichen",
        scope: "web",
        tickets: ["fsmf"],
      },
    ],
  },
  {
    version: "0.6.38",
    entries: [
      {
        kind: "fix",
        text: "Line-Tool zeigt nur noch EIN Längenfeld — das blaue Live-Input",
        scope: "web",
        tickets: ["c2ox"],
      },
    ],
  },
  {
    version: "0.6.37",
    entries: [
      {
        kind: "fix",
        text: "Transform-Feature-Gizmo committet in eigene Params; Längen-Einheit mm",
        scope: "web,kernel",
        tickets: ["p3vh"],
      },
    ],
  },
  {
    version: "0.6.36",
    entries: [
      {
        kind: "feat",
        text: "Onshape-Selektion — runde Sphere-Tönung + vollflächiger Face-Fill",
        scope: "web",
        tickets: ["por8"],
      },
    ],
  },
  {
    version: "0.6.35",
    entries: [
      {
        kind: "fix",
        text: "Query-Auflösung auf parametrischen Weave + robustes Face-Profil",
        scope: "brep",
        tickets: ["vyee"],
      },
    ],
  },
  {
    version: "0.6.34",
    entries: [
      {
        kind: "fix",
        text: "Face-Extrude-Profil per Endpunkt-Nähe zu Ringen ordnen",
        scope: "brep",
        tickets: ["oa9h"],
      },
    ],
  },
  {
    version: "0.6.33",
    entries: [
      {
        kind: "fix",
        text: "SVG-Import-Lexer für gepackte Pfaddaten",
        scope: "web",
        tickets: ["bgvh"],
      },
    ],
  },
  {
    version: "0.6.32",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Gizmo poliert — S/W-Pfeil, smooth Drag, echte Preview",
        scope: "web",
        tickets: ["qqgo"],
      },
    ],
  },
  {
    version: "0.6.31",
    entries: [
      {
        kind: "fix",
        text: "B-Rep-Basiskanten als Fat-Lines rendern",
        scope: "web",
        tickets: ["8f5z"],
      },
    ],
  },
  {
    version: "0.6.30",
    entries: [
      {
        kind: "feat",
        text: "Extrude-Tiefen-Gizmo mit Pfeil + transluzenter Slab-Vorschau",
        scope: "web",
        tickets: ["jn3t"],
      },
    ],
  },
  {
    version: "0.6.29",
    entries: [
      {
        kind: "fix",
        text: "Extrude-Toolbar-Button wirkt auf selektierte Flächen",
        scope: "web",
        tickets: ["981k"],
      },
    ],
  },
  {
    version: "0.6.28",
    entries: [
      {
        kind: "feat",
        text: "geschlossene Sketch-Konturen als Flächen schattiert + Wheel-Zoom bei aktivem Tool",
        scope: "web",
        tickets: ["7x4v"],
      },
    ],
  },
  {
    version: "0.6.27",
    entries: [
      {
        kind: "feat",
        text: "Onshape-Multiselect + Rechteck-Selektion, Kugel-Highlight",
        scope: "web",
        tickets: ["select", "rectan", "gle"],
      },
    ],
  },
  {
    version: "0.6.26",
    entries: [
      {
        kind: "feat",
        text: "Viewport-Selektion in Seitenleiste spiegeln + Default-Geometrie selektierbar",
        scope: "web",
        tickets: ["r36f"],
      },
    ],
  },
  {
    version: "0.6.25",
    entries: [
      {
        kind: "feat",
        text: "Attachments im Report-Issue-Dialog",
        scope: "web,server",
        tickets: ["8onw"],
      },
    ],
  },
  {
    version: "0.6.24",
    entries: [
      {
        kind: "fix",
        text: "verdeckte Kanten nicht pickbar, Vertex-Punkte pixelgenau",
        scope: "web",
        tickets: ["twvd"],
      },
    ],
  },
  {
    version: "0.6.23",
    entries: [
      {
        kind: "fix",
        text: "Kanten-Hover heller im Selektions-Farbton",
        scope: "web",
        tickets: ["m0yw"],
      },
    ],
  },
  {
    version: "0.6.22",
    entries: [
      {
        kind: "feat",
        text: "Onshape-Booleans — Add/New/Remove/Intersect statt Union/Difference/Intersect",
        scope: "kernel,web",
      },
    ],
  },
  {
    version: "0.6.21",
    entries: [
      {
        kind: "feat",
        text: "Controls-Presets — Maus-/Tastatur-Schemata anderer CAD-Programme",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.20",
    entries: [
      {
        kind: "fix",
        text: "Path-Profile analytisch liften — gerade Extrude-Kanten",
        scope: "brep",
        tickets: ["9zlq"],
      },
    ],
  },
  {
    version: "0.6.19",
    entries: [
      {
        kind: "feat",
        text: "Vertex-/Flächen-/Körper-Selektion im Viewport",
        scope: "web",
        tickets: ["evq2"],
      },
    ],
  },
  {
    version: "0.6.18",
    entries: [
      {
        kind: "fix",
        text: "sub-band grazing seams weave exactly",
        scope: "brep",
        tickets: ["g4rr"],
      },
    ],
  },
  {
    version: "0.6.17",
    entries: [
      {
        kind: "fix",
        text: "Kanten-Highlight folgt der Auswahl",
        scope: "web",
        tickets: ["twxw"],
      },
    ],
  },
  {
    version: "0.6.16",
    entries: [
      {
        kind: "fix",
        text: "Winkelfeld aus Line-Live-Input-HUD entfernen",
        scope: "web",
        tickets: ["730r"],
      },
    ],
  },
  {
    version: "0.6.15",
    entries: [
      {
        kind: "fix",
        text: "ElementInfo-Readout selektierbar machen",
        scope: "web",
        tickets: ["ijqe"],
      },
    ],
  },
  {
    version: "0.6.14",
    entries: [
      {
        kind: "feat",
        text: "parametric weave als Default an",
        scope: "brep",
        tickets: ["el1x"],
      },
    ],
  },
  {
    version: "0.6.13",
    entries: [
      {
        kind: "fix",
        text: "edge hover highlight uses selection colour, 1px",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.12",
    entries: [
      {
        kind: "fix",
        text: "selected edge as real 2px fat line, not invisible 1px",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.11",
    entries: [
      {
        kind: "fix",
        text: "selected-edge highlight offset, now gold-orange 2px",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.10",
    entries: [
      {
        kind: "feat",
        text: "close report dialog on submit, no confirmation step",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.9",
    entries: [
      {
        kind: "feat",
        text: "capture full app window in issue tickets, not just viewport",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.8",
    entries: [
      {
        kind: "feat",
        text: "in-app issue/feature ticket button",
        scope: "web",
      },
    ],
  },
  {
    version: "0.6.7",
    entries: [
      {
        kind: "fix",
        text: "split-on-spurious corner resolution for grazing primitives",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.6",
    entries: [
      {
        kind: "fix",
        text: "seed every active arc of a trimmed SSI component",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.5",
    entries: [
      {
        kind: "feat",
        text: "Phase 2b-iii — Seed-Pfad in weave_parametric verdrahtet",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.4",
    entries: [
      {
        kind: "feat",
        text: "Phase 2b-ii — build_seed_wireframe komplett",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.3",
    entries: [
      {
        kind: "feat",
        text: "Phase 2b-i — Seed-Weaver-Skelett + Scope-Gate",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.2",
    entries: [
      {
        kind: "feat",
        text: "Phase 2a — Aktiv-Intervall-Marsch im Tracer",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.1",
    entries: [
      {
        kind: "feat",
        text: "Phase 1b-ii — Branch-Pair-Seeds + SSI-Fixes",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.6.0",
    entries: [
      {
        kind: "feat",
        text: "Phase 1b-i — SSI-Komponentenzählung + Seeds",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.99",
    entries: [
      {
        kind: "feat",
        text: "Phase 1a — Branch-Paar-Aufzählung + AABB-Prune",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.98",
    entries: [
      {
        kind: "feat",
        text: "Phase 1a Fundament — statische CSG-Branch-Enumeration",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.97",
    entries: [
      {
        kind: "feat",
        text: "Differential-Komparator (Phase 0c Parametric-Weaving)",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.96",
    entries: [
      {
        kind: "feat",
        text: "Force-Pure-Octree-Modus (Phase 0b Parametric-Weaving)",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.95",
    entries: [
      {
        kind: "feat",
        text: "DL-24 Manifold-/Watertight-Invariante (Phase 0a Parametric-Weaving)",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.93",
    entries: [
      {
        kind: "feat",
        text: "DL-81 krümmungsadaptives Edge-Tracing in Produktion verdrahtet",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.92",
    entries: [
      {
        kind: "feat",
        text: "DL-81 krümmungsadaptives Edge-Tracing hinter Flag",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.91",
    entries: [
      {
        kind: "feat",
        text: "feature-adaptive octree — Face-Zellen gröber als Edge/Vertex",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.90",
    entries: [
      {
        kind: "fix",
        text: "Kanten-Overlay zeigt NURBS-Strukturpunkte statt Tessellierung",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.5.89",
    entries: [
      {
        kind: "feat",
        text: "Settings-Overlays für Kantenpunkte und Octree-Zellen",
        scope: "debug",
      },
    ],
  },
  {
    version: "0.5.87",
    entries: [
      {
        kind: "fix",
        text: "kernel-staleness guard compares vs on-disk WASM, not package.json",
        scope: "build",
      },
    ],
  },
  {
    version: "0.5.86",
    entries: [
      {
        kind: "fix",
        text: "DL-81 — drop phantom triple junction that kinks straight box edges",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.85",
    entries: [
      {
        kind: "fix",
        text: "DL-79 Kanten-Rescue + DL-80 geschlossene Loop-NURBS",
        scope: "brep",
      },
    ],
  },
  {
    version: "0.5.79",
    entries: [
      {
        kind: "feat",
        text: "live constraint solving during grip drag",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.78",
    entries: [
      {
        kind: "feat",
        text: "point-on-spline solver constraint for open B-splines",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.77",
    entries: [
      {
        kind: "feat",
        text: "make ellipse a first-class solver entity with tangent/point-on constraints",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.76",
    entries: [
      {
        kind: "fix",
        text: "use authoritative solver dof() instead of JS estimate",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.72",
    entries: [
      {
        kind: "feat",
        text: "editable sketch_path vertices + rounded SVG rects",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.71",
    entries: [
      {
        kind: "feat",
        text: "G1 endpoint tangent legs for interpolation-mode splines",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.70",
    entries: [
      {
        kind: "feat",
        text: "G1 tangent at B-spline endpoints via control-polygon legs",
        scope: "sketch",
      },
    ],
  },
  {
    version: "0.5.69",
    entries: [
      {
        kind: "feat",
        text: "real cubic B-spline (control-point mode, de Boor, Boehm lowering)",
      },
    ],
  },
  {
    version: "0.5.68",
    entries: [
      {
        kind: "feat",
        text: "filled glyph text from real fonts (opentype.js) replaces single-stroke",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.67",
    entries: [
      {
        kind: "feat",
        text: "exact-Bézier SVG import + full transform stack + sketch_path primitive",
      },
    ],
  },
  {
    version: "0.5.66",
    entries: [
      {
        kind: "feat",
        text: "chain/contour offset replaces single-curve offset",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.65",
    entries: [
      {
        kind: "feat",
        text: "text as sketch geometry via a single-stroke font",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.64",
    entries: [
      {
        kind: "feat",
        text: "interpolating spline tool",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.63",
    entries: [
      {
        kind: "feat",
        text: "import SVG and DXF vector files as sketch geometry",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.62",
    entries: [
      {
        kind: "feat",
        text: "use/project external geometry into the sketch",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.61",
    entries: [
      {
        kind: "feat",
        text: "conflict/redundancy diagnostics in the constraints panel",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.60",
    entries: [
      {
        kind: "feat",
        text: "selectable elements/constraints panel in the sketcher",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.59",
    entries: [
      {
        kind: "feat",
        text: "live numeric input while drawing a line",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.58",
    entries: [
      {
        kind: "feat",
        text: "move / copy / rotate selected sketch geometry",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.57",
    entries: [
      {
        kind: "feat",
        text: "sketch linear & circular pattern",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.56",
    entries: [
      {
        kind: "feat",
        text: "interactive sketch offset",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.55",
    entries: [
      {
        kind: "feat",
        text: "interactive sketch mirror across a picked axis",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.54",
    entries: [
      {
        kind: "feat",
        text: "sketch split — cut a curve in two at the click, keep both",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.53",
    entries: [
      {
        kind: "feat",
        text: "sketch extend — lengthen a line to the nearest curve",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.52",
    entries: [
      {
        kind: "feat",
        text: "sketch chamfer — bevel a corner between two lines",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.51",
    entries: [
      {
        kind: "feat",
        text: "sketch fillet — round a corner between two lines",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.50",
    entries: [
      {
        kind: "feat",
        text: "slot (stadium) tool",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.49",
    entries: [
      {
        kind: "feat",
        text: "complete the ellipse tool UI",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.48",
    entries: [
      {
        kind: "feat",
        text: "rectangle-center, rectangle-3point, circle-3point tools",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.47",
    entries: [
      {
        kind: "feat",
        text: "regular polygon tool",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.46",
    entries: [
      {
        kind: "feat",
        text: "live DoF counter in the sketch HUD",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.45",
    entries: [
      {
        kind: "feat",
        text: "box-select / marquee in the sketcher",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.44",
    entries: [
      {
        kind: "feat",
        text: "point-on-object constraint",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.43",
    entries: [
      {
        kind: "feat",
        text: "fix/lock constraint pins a sketch point",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.42",
    entries: [
      {
        kind: "feat",
        text: "concentric constraint in sketch palette",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.41",
    entries: [
      {
        kind: "feat",
        text: "make rectangle & polygon edges trimmable",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.40",
    entries: [
      {
        kind: "feat",
        text: "real interactive Trim for the 2D sketcher",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.39",
    entries: [
      {
        kind: "feat",
        text: "construction (reference) geometry for the 2D sketcher",
      },
    ],
  },
  {
    version: "0.5.38",
    entries: [
      {
        kind: "feat",
        text: "radius + diameter dimensions for circles/arcs",
      },
    ],
  },
  {
    version: "0.5.37",
    entries: [
      {
        kind: "feat",
        text: "midpoint + symmetric constraints in the sketch palette",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.36",
    entries: [
      {
        kind: "feat",
        text: "wire mirror + offset profile-operation tools",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.35",
    entries: [
      {
        kind: "feat",
        text: "interactive arc drawing in the 2D sketch editor",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.33",
    entries: [
      {
        kind: "feat",
        text: "two-finger pinch-to-zoom for the 2D sketch canvas",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.32",
    entries: [
      {
        kind: "feat",
        text: "mobile touch hit-targets for toolbar + bottom tabs",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.31",
    entries: [
      {
        kind: "feat",
        text: "mobile responsive Outliner drawer",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.30",
    entries: [
      {
        kind: "feat",
        text: "grips for rectangle/polygon/arc geometry",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.29",
    entries: [
      {
        kind: "feat",
        text: "P0.3 drag-edit committed sketch geometry via grips",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.27",
    entries: [
      {
        kind: "feat",
        text: "editable dimensions — Length/Distance + Angle",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.26",
    entries: [
      {
        kind: "feat",
        text: "selectable inner-wall + opening B-Rep faces",
        scope: "shell",
      },
      {
        kind: "feat",
        text: "reach + verify the sketch constraint palette",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.25",
    entries: [
      {
        kind: "feat",
        text: "explicit constraint palette in the sketch editor",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.24",
    entries: [
      {
        kind: "feat",
        text: "click-select committed sketch geometry in select mode",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.23",
    entries: [
      {
        kind: "feat",
        text: "add sketch selection model for committed geometry",
        scope: "web",
      },
    ],
  },
  {
    version: "0.5.22",
    entries: [
      {
        kind: "feat",
        text: "wire all sketch constraint types into the live solver",
        scope: "web",
      },
    ],
  },
  {
    version: "0.4.9",
    entries: [
      {
        kind: "feat",
        text: "add Render loop demand/always toggle in Settings",
        scope: "web",
      },
    ],
  },
  {
    version: "0.4.8",
    entries: [
      {
        kind: "perf",
        text: "drive WebGPU viewport on demand instead of always",
        scope: "web",
      },
    ],
  },
  {
    version: "0.4.6",
    entries: [
      {
        kind: "fix",
        text: "unstick WebGPU swapchain present with a real pixel-size kick",
        scope: "web",
      },
    ],
  },
  {
    version: "0.4.5",
    entries: [
      {
        kind: "fix",
        text: "stop shader-compile freeze + reliable present after compile",
        scope: "web",
      },
    ],
  },
  {
    version: "0.4.1",
    entries: [
      {
        kind: "feat",
        text: "SDF shade pass writes canvas depth for B-Rep edge occlusion",
      },
    ],
  },
];
