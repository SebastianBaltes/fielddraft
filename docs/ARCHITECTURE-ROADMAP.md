# FieldDraft — Architektur-Roadmap (geometrischer Kern)

> **Diese Datei.** Die erkannten **architektonisch-geometrischen Probleme** des
> Kerns — Dinge, die keine Bugs und keine Feature-Slices sind, sondern fehlende
> oder unsaubere symbolische Behandlung geometrischer Sachverhalte. Abgrenzung:
> `docs/ROADMAP.md` ist die Produkt-/Slice-Roadmap (was gebaut wird),
> `STATUS.md` der Ist-Stand. Dieses Dokument ordnet die strukturellen
> Geometrie-Themen nach Dringlichkeit und benennt pro Thema Problem,
> Lösungsrichtung und Auslöser (wann es real wird).
>
> Stand: 2026-06-11. Entstanden aus dem Architektur-Review zu Ticket o2j9.

## Leitregel (gilt für alle Punkte)

Degenerierte geometrische Lagen (Koinzidenz, Tangenz, Berührung) werden
**symbolisch bzw. analytisch auf Konstruktionsdaten entschieden, niemals durch
Sampling-Epsilons**. Sampling beantwortet kontinuierliche Fragen (wo ist die
Grenze?) robust; diskrete Fakten („diese zwei Flächen sind dieselbe", „diese
zwei Flächen berühren sich tangential") sind auf Sample-Ebene fundamental
schlecht konditioniert und gehören auf die Branch-/Feature-/Sketch-Ebene, wo
sie exakt vorliegen.

## Die Kontaktordnungs-Leiter

Die Punkte 1 und 2 sind zwei Sprossen derselben Leiter — Kontakt zweier
Trägerflächen, klassifiziert nach Ordnung:

- **Ordnung 0 — transversal:** Flächen schneiden sich, Gradienten unabhängig.
  Der Normalfall; SSI-Tracer und Weave tragen ihn heute.
- **Ordnung 1 — tangential entlang einer Kurve:** Flächen berühren sich G1
  (∇f_a ∥ ∇f_b entlang der ganzen Kurve). Fillets, G1-Spline-Stöße,
  Zylinder-auf-Ebene. Punkt 2.
- **Ordnung ∞ — koinzident entlang einer Fläche:** identische Trägerfläche.
  Koplanare/koaxiale Stapel. Punkt 1.

Je höher die Ordnung, desto breiter der Toleranzschlauch, in dem Sampling
beide Flächen „gleichzeitig sieht" (linear → wurzelförmig → flächig), und
desto zwingender die symbolische Entscheidung. Die Koinzidenz-Registry
(Punkt 1, Phase 1) wird deshalb mit einem **Kontaktordnungs-Feld** designt,
nicht als Bool — Ordnung 1 dockt später an dieselbe Stelle an.

---

## 1. Koinzidente Trägerflächen (koplanar / koaxial) — KERN UMGESETZT

**Problem.** Zwei Branches auf derselben Trägerfläche (bündige Box-Stapel,
koplanare Extrude-Stapel, koaxial gleichradige Zylinder) brechen die stillen
Annahmen der Branch→Face-Attribution in `finish_weave` (Injektivität und
Totalität der Abbildung Branch → Grenzregion): Geister-Membranen, duplizierte
Rim-Kanten, koplanare Wände ohne Naht — Euler −16 statt 2 (Ticket o2j9).
Gleiche Wurzel hinter dem Octree-Kontributor-Schmieren (oa9h/vyee).

**Lösung.** Klassifikationsstufe IN/ON-same/ON-opposite mit analytischer
Koinzidenz-Registry. **Plan-SSOT: `docs/BREP-CLASSIFICATION-PLAN.md`**
(Phasen 0–4: Invarianten-Gate → Registry → Interior-Cull →
CoincidentSurface-Gate im Seeding → Octree auf Klassen). Deckt auch
die Null-Dicke-Semantik bei bündigem `remove` ab (ON-opposite).

**Status.** Phasen 0–3 umgesetzt (v0.6.58–0.6.63), Ticket o2j9 geschlossen:
Gate, Registry, Kanonikalisierung in `finish_weave`, Koinzidenz-Gate in
`seed_branch_pairs`, On-Boundary-Gate in `both_branches_active`,
Interior-Fin-Beweis. Das ursprünglich geplante 2D-Arrangement war nicht
nötig (Naht-Kanten kommen aus den transversalen Paaren mit den berandenden
Flächen). Offen: Phase 4 (Octree-Route auf Klassen — Produktionspfad ist die
Seed-Route) und F0-1 (Face-pro-Region).

## 2. Tangentialer Kontakt (Kontaktordnung 1) — ZURÜCKGESTELLT, Design-Direktive aktiv

**Problem.** Berühren sich zwei Trägerflächen G1-stetig entlang einer Kurve,
degeneriert die SSI: die Marschrichtung des Tracers ist ∇f_a × ∇f_b, und die
verschwindet entlang der **gesamten** Kante. Zusätzlich trennen sich die
Flächen quadratisch statt linear — der Toleranzschlauch um die Kurve ist
wurzelförmig breit, numerisches Nachschärfen ist dort prinzipiell schlecht
konditioniert. Tangentiale SSI ist das kanonisch harte Problem aller
BRep-Kernel; eine robuste rein numerische Allgemeinlösung existiert nicht.

**Das ist nicht fillet-spezifisch.** Jeder G1-Stoß zweier Bodies erzeugt die
Lage: zwei Revolves um dieselbe Achse, deren Profile sich einen Endpunkt mit
gleicher Tangente teilen, ergeben eine tangentiale Kreis-Kante — identisch
zur Fillet-Rail. Dasselbe bei Extrudes mit G1 anschließenden Profilkurven.
Fillets sind nur der Fall, der die Lage *garantiert* erzeugt.

**Lösungsrichtung: konstruktive Provenienz statt besserem Tracing.**
Tangenz entsteht in CAD praktisch nie zufällig, sondern wird konstruiert —
und die Konstruktion weiß beides:

- **Dass** der Kontakt tangential ist: entweder als Sketch-Constraint
  (Tangent ist ein gelöster Constraint, exakt bekannt) oder durch exakten
  Vergleich der Profil-Endtangenten auf den 2D-Sketchdaten — eine
  1D/2D-Entscheidung auf Konstruktionsdaten, robust, kein 3D-Sampling.
- **Wo** die Kante liegt: geteilter Profilpunkt unter Revolve → Kreis,
  unter Extrude → Strecke. Exakte analytische Geometrie, kein Tracing nötig.

Die Kante wird also symbolisch materialisiert (als „smooth edge", Diederwinkel
180° — topologisch nötig als Face-Grenze, geometrisch glatt; STEP kennt das),
der Tracer wird nie auf die degenerierte Kurve angesetzt. Das ist derselbe
architektonische Zug wie die Koinzidenz-Registry, eine Sprosse tiefer auf der
Leiter. Für *unkonstruierte* Tangenz analytischer Paare (Zylinder liegt auf
Ebene: Abstand Achse–Ebene = Radius) entscheidet die Registry analytisch;
zufällige Tangenz freier NURBS-Flächen bleibt deklarierte Limitation (sie ist
maßnull — wer sie trifft, hat passende Zahlen eingetippt, also konstruiert).

Numerischer Vollständigkeit halber: Tracing zweiter Ordnung existiert (die
Kurventangente folgt aus den Nullrichtungen der Differenz der zweiten
Fundamentalformen; Ye/Maekawa, Kriezis/Patrikalakis), braucht Hessians und
degeneriert bei G2-Kontakt erneut — letzte Reserve, kein Fundament. Es
widerspricht außerdem dem Geist der Leitregel.

**Entscheidung (User, 2026-06-11).** Fillets sind nachgelagerte Features, auf
deren BRep selten weitere Features aufbauen; die SDF deckt die Anwendungsfälle
(z. B. Bohrung in ein Fillet) bereits ab. Das Thema wird **zurückgestellt** —
aber weil Tangenz allgemeiner ist als Fillets, gilt ab sofort die
**Design-Direktive**: die Koinzidenz-Registry (Punkt 1, Phase 1) modelliert
Kontakt als Ordnung (0/1/∞), nicht als Bool, sodass Ordnung 1 später ohne
Redesign andockt.

**Auslöser.** Erster Nutzer-Fall eines G1-Stoßes mit BRep-Bedarf (STEP-Export,
Face-Picking auf der Naht) oder Beginn der Fillet-BRep-Arbeit aus
`BREP-FEATURE-VISION.md`. Heutiger Schaden begrenzt: Mesh/Render sind
unbetroffen (G1-Stoß ist glatt — fürs Meshing ist „kein scharfes Feature"
sogar korrekt), es leiden Wireframe/Picking/STEP.

## 3. Epsilon-Politik: eine zentrale, skalenrelative Toleranz-Policy — UMGESETZT

**Status (2026-07-07).** Kern durch Phase 5a des Klassifikations-Plans
(`brep::tol`, v0.6.71–74): benannte, klassifizierte Toleranzen
(skalenrelativ / dimensionslos / eingefroren-absolut mit
Konvertierungsauftrag). Kernel-weites Audit 2026-07-07: Restbestand
klassifiziert (dimensionslos / Sketch-mm-Domäne / Meshing-Export-Domäne —
Details im `brep/tol.rs`-Modulkopf), skalen-fragile Nachzügler migriert
(AABB-Docking-Slack in `body.rs` jetzt span-relativ mit 1-µm-Floor; das
Naming-Sweep-ε ist seit dem Persistent-Naming-N1 gruppen-relativ).
Konvention für neue geometrische Längen-Toleranzen: über `brep::tol`
ankern.

**Problem.** Der Kern mischt absolute Toleranzen (`PROJECT_TOL = 1e-9`,
`BISECT_ACTIVE_TOL = 1e-7`, `PARALLEL_EPS = 1e-6`) mit diagonal-relativen
(Cluster-Gaps, Tracer-Schrittweiten). Verhalten ändert sich dadurch mit der
Modellgröße — oa9h/vyee kippten ab ≳100 mm. Jede neue absolute Konstante
verschärft das.

**Lösung.** Eine zentrale Toleranz-Policy (Modul), die alle geometrischen
Epsilons aus **einer** skalenrelativen Basis ableitet (Modell- oder
Body-Diagonale), mit dokumentierter Bedeutung pro abgeleiteter Größe.
Bestehende Konstanten dorthin migrieren; neue absolute Literale per Konvention
(und idealerweise Lint/Test) verbieten.

**Auslöser.** Guter eigenständiger Hygiene-Schritt **nach** Punkt 1 — die
Klassifikationsstufe entfernt erst die Epsilon-Konsumenten, die Koinzidenz
heute kompensieren; danach ist der verbleibende Bestand klein und migrierbar.

## 4. Persistentes Naming: Provenienz statt räumlicher Reihenfolge — KERN UMGESETZT

**Status (2026-07-07, v0.6.448–450).** Plan-SSOT:
`docs/plans/PERSISTENT-NAMING-PLAN.md`, Phasen N0–N4 umgesetzt: Referenz-
Auflösung bewertet alle Kandidaten einheitlich (Fingerprint + Hash-Bonus,
provenienz-gebunden auf die Signatur-Gruppe — kein stilles Fehlbinden,
ehrliches `Ambiguous`), Element-Referenzen tragen einen persistierten
Fingerprint (`qFace(<hash> @ …)`), alle Feature-Pfade heilen über EINEN
Resolver, und Heal-on-Save re-kanonisiert gespeicherte Referenzen. Die
Sweep-Ordnung ist gruppen-relativ statt absolut-ε. `spatial_index` bleibt
als letzter Tie-Breaker in der ID — die Referenz-Schicht kompensiert sein
Kippen. Offene Follow-ups im Plan-Doc.

**Problem.** `face_id(branch, spatial_index)` disambiguiert mehrere Faces
desselben Branches über die räumliche Sortierung der Weave-Ausgabe. Die kippt
unter µm-Verschiebungen und unter Topologie-Änderungen (Ring-Face nach
Phase 3 von Punkt 1) — beobachtete Folge: „face not found" nach harmlosen
Edits. Fingerprint-Healing mildert, trägt aber nicht als Fundament.

**Lösung.** Persistente Identität auf **Branch-/Feature-Provenienz plus
Koinzidenz-Klassen** gründen (welches Feature, welcher Profilabschnitt,
welche Klasse), `spatial_index` nur noch als letzter deterministischer
Tie-Breaker innerhalb echter Symmetrie. Die Registry aus Punkt 1 liefert
dafür die fehlende Vokabel („diese Face ist der nicht überdeckte Teil von
Branch X in Klasse K").

**Auslöser.** Spätestens mit Phase 3 von Punkt 1 (Ring-Faces verschieben
`spatial_index`); das dortige Phase-0-Gate enthält Handle-Resolve-Tests, die
den Bedarf messbar machen.

## 5. Kissing Contact und die Mannigfaltigkeits-Policy

**Problem.** Berühren sich zwei Körper exakt in einem Punkt oder einer Kante,
ist die Union **topologisch** nicht-mannigfaltig — das kann keine Pipeline
„richtig" weben. Heute undefiniertes Verhalten; gleichzeitig ist offen, ob
die BRep-Datenstruktur strikt 2-mannigfaltig bleiben soll oder benannte
non-manifold-Stellen tragen können muss.

**Lösung.** Policy statt Heuristik (so im Klassifikations-Plan verankert):
Weave liefert das Ergebnis ohne Naht (zwei Shells eines Solids),
`manifold_violations()` benennt die Stelle, ein Test dokumentiert das
Verhalten. Die Datenstruktur-Frage wird damit beantwortbar: strikt
2-mannigfaltig pro Shell, Berührung = Shell-Grenze — keine
non-manifold-Erweiterung der Halfedge-Struktur nötig, solange kein Feature
sie verlangt.

**Auslöser.** Phase 0 von Punkt 1 enthält den kantenberührenden Box-Fall als
Policy-Test; damit ist das Thema dort miterledigt, solange kein Feature
non-manifold-Ergebnisse weiterverarbeiten will.

## 6. min/max-CSG ist nur C⁰ — Richtlinie für Gradient-Konsumenten

**Problem.** Der CSG-Baum kombiniert per min/max: das Feld ist stetig, aber
der Gradient springt an Äquidistanz-Flächen **im Inneren** des Materials, und
min/max unterschätzt die echte Distanz (kein exaktes SDF). Logik, die
Gradienten oder Distanzwerte abseits der Oberfläche konsumiert
(Kontributor-Tests, Step-Größen, Normalen-Schätzer), kann dort fehlschließen.

**Lösung.** Kein Umbau (exakte SDFs für CSG gibt es nicht in geschlossener
Form), sondern eine **dokumentierte Richtlinie**: Gradienten/Distanzen sind
nur in Oberflächennähe des *dominanten* Branches belastbar; Innere-Logik muss
branch-bewusst auswerten (den Branch direkt fragen, nicht den Baum). Bestehende
Konsumenten einmalig auditieren, neue an der Richtlinie messen.

**Auslöser.** Laufend; konkret bei jedem neuen Konsumenten von ∇f oder |f|
abseits der Oberfläche (z. B. Punkt 1 Phase 2 nutzt deshalb bewusst den
±ε-Vorzeichentest am *bekannten analytischen* Flächenpunkt — wohlkonditioniert).

---

## Nicht auf dieser Roadmap

- **Face-Merging** koplanarer Nachbarfaces: Kosmetik, bewusst kein Ziel
  (siehe `BREP-CLASSIFICATION-PLAN.md`, Zielbild).
- **Exakte Arithmetik / Perturbation** als Robustheitsfundament: passt nicht
  zur SDF-Architektur; die Leitregel (symbolische Entscheidung auf
  Konstruktionsdaten) ist unser Äquivalent.
