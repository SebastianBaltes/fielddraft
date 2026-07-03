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
