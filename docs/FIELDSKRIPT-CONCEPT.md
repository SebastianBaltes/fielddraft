# FieldSkript — Konzept für laufzeit-skriptbare Features

> Status: Konzept (2026-06-01). Baut auf der **§4.6-Vision** (Feature-as-Function,
> Regeneration-Worker, Dual-API), dem Query-Minimum (`queries.rs`, K4.5.7) und
> **DL-61** (Feature = Envelope um einen offenen `CsgNode`) auf.
> Referenz: Onshape FeatureScript-Doku — [Feature UI](https://cad.onshape.com/FsDoc/uispec.html),
> [Feature types](https://cad.onshape.com/FsDoc/feature-types.html),
> [Values & types](https://cad.onshape.com/FsDoc/variables.html).

## 0. Worum es geht

FieldSkript ist die **laufzeit-definierbare** Schwester der in Rust kompilierten
Built-in-Features. Ein FieldSkript ist eine **TypeScript-Funktion**, die – wie ein
Onshape-Feature – `(context, id, definition)` empfängt, ihre Parameter über eine
**Meta-Auszeichnung** deklariert und über eine `fd.*`-API **Geometrie-Knoten emittiert**
(DL-61: ein `CsgNode`-Subtree). Damit braucht ein neues Feature **keinen Rust-Build und
keine Änderung an einem zentralen Datentyp** mehr.

**Markenregel (README §11):** keine Onshape-Markenbegriffe in produkt-/API-sichtbaren
Namen. Die API heißt `fd.*`, nicht `featureScript.*`. „Feature Studio" → **Feature-Bibliothek**,
„FeatureScript" → **FieldSkript**, „Part Studio"/„Mate Connector" werden generisch ersetzt.

### Bestehende Bausteine, auf die wir aufsetzen

| Baustein | Datei / Ort | Stand |
|---|---|---|
| **Dual-API** (Regeneration-Worker) | `web/src/workerSandbox.ts`, `outerApi.ts`, `workerRpc.ts` | Inner-API `globalThis.fd` injiziert, Outer-API-RPC steht; nur Probe-Methoden (`version`/`log`/`ping`/`echo`). Feature-Dispatch = geplanter Slice **K4.5.8** |
| **Determinismus-Sandbox** | `workerSandbox.ts` | `freezeGlobals` (kein `fetch`/Timer, `Date.now→0`), `reseedRandom` (mulberry32, doc-hash-seeded) — fertig |
| **Query-Minimum** | `crates/kernel/src/queries.rs` | `QueryExpr {CreatedBy, FaceOf, EdgeBetween}`, Parser, **Evaluator (Feature-Ebene)**, `QueryCache` (versions-keyed). Noch **nicht** in `Context` verdrahtet, noch **keine** B-Rep-Element-Auflösung |
| **Query-Wert-Typ** | `crates/protocol/src/lib.rs` (`Query`, `MultiQuery`) | als Parameter-Typ im Schema-Macro vorhanden |
| **Schema-getriebene Dialoge** | `crates/features-macros` → `web/src/widgets/schema.ts` → `FeatureDialog` | Built-ins erzeugen Schema-JSON (Compile-Zeit), UI rendert **generisch**. FieldSkript erzeugt **dasselbe** Schema-JSON zur Laufzeit |
| **Knoten-IR + Provenienz** | `csg.rs` (`CsgNode`, `BranchId`), Naming (DL-31…DL-60) | offene kompositionelle IR + stabile FNV-1a-IDs — das Substrat, das ein robustes Query-System braucht |
| **`@param`-Skript-Muster + CRUD/Editor-Vorbild** | `web/src/fieldscript.ts` (P2.8) | **separates** System für Assembly-Kinematik (Bewegung, nicht Modellierung). Liefert das Muster für Param-Header + Skript-Persistenz, das FieldSkript verallgemeinert |

> **Zwei Skript-Systeme, klar getrennt halten:** `fieldscript.ts` (P2.8) steuert
> **Bewegung** von Mates zur Laufzeit (kein Geometrie-Output). FieldSkript (dieses
> Konzept) erzeugt **Modellgeometrie** im Regenerations-Pfad. Sie teilen sich später
> Editor + Param-UI, aber nicht den Ausführungskontext.

---

## 1. Was wir von Onshape übernehmen — und was bewusst anders ist

**Übernommen (bewährte Muster):**
- **Feature = Funktion** `(context, id, definition)` mit getrennter **Parameter-Deklaration**
  (Onshape: `precondition`) und **Body** (Geometrie-Emission).
- **Meta-Annotation der Parameter** treibt zugleich Typprüfung **und** Dialog-UI.
- **Queries statt roher IDs**: Geometriereferenzen sind **lazy, regenerationsfeste**
  Prädikate über Topologie, nicht eingefrorene Mesh-Indizes. (Onshape: „queries are lazily
  run, they don't give the user data which can go out of date".)
- **`id + "sub"`-Verkettung** zur Verfolgung von Teil-Operationen eines Features.
- **Determinismus** als harte Invariante (gleiche Eingabe → bit-identisches Ergebnis).

**Bewusst anders:**
- **Sprache: TypeScript** statt einer Eigenbau-DSL. Onshape musste eine Sprache erfinden
  (2014, kein guter Embeddable-Kandidat); wir bekommen mit TS Tooling, Typsystem und
  Monaco-IntelliSense geschenkt. Die „Annotation"-Syntax (`annotation {…}` als
  Sprach-Konstrukt) ersetzen wir durch eine **deklarative, serialisierbare TS-DSL**
  (Builder-Funktionen), die in **dasselbe Schema-JSON** mündet, das unsere Dialoge schon rendern.
- **Geometrie-Kern: SDF/`CsgNode`** statt B-Rep-Kernel (Parasolid). Ein FieldSkript emittiert
  einen `CsgNode`-Subtree (DL-61); B-Rep wird daraus **abgeleitet** (Octree-Weben), nicht
  primär erzeugt. Queries lösen gegen die abgeleitete B-Rep + Provenienz auf.
- **Kein serverseitiger Interpreter**: FieldSkript läuft **client-seitig** im
  Regeneration-Worker (CLAUDE.md: Backend optional). Determinismus-Sandbox statt
  Server-Isolierung.

---

## 2. Laufzeit-Architektur: Rust treibt, FieldSkript ist Callback

### 2.0 Entscheidung (Auflösung von §12.1)

**Der Regenerations-Treiber bleibt in Rust.** FieldSkript wird als **Callback** eingeklinkt,
nicht als neuer Schleifenbesitzer. Begründung — die zwei Zahlen ziehen in dieselbe Richtung:

- **95 % aller Feature-*Ausführungen* zur Laufzeit sind native Built-ins** (Box, Extrude,
  Fillet … — das, was in jedem Dokument tausendfach regeneriert wird). Der **heiße Pfad muss
  nativ und grenzfrei** bleiben: Rust ruft Rust direkt, ohne WASM↔JS-Hop.
- **99 % aller *existierenden* Feature-Typen werden FieldSkript sein** (der lange Katalog-Schwanz
  benutzerdefinierter Composite-Features). Die müssen *pluggbar* sein, aber jede einzelne
  Instanz ist selten und gecacht.

Die Schleife gehört also dorthin, wo der heiße Pfad ist (Rust). Die in §2.0 der vorherigen
Fassung erwogene **Treiber-Inversion nach TS** (Alternative B) ist damit **verworfen**: Sie
hätte den 95-%-Pfad mit Grenzübertritten belastet und das Headless-/Server-Regenerieren an
einen JS-Runtime gekettet, um den 1-%-Fall (Skripte, die Queries/Sub-Features brauchen)
bequemer zu machen. Stattdessen lösen wir diesen 1-%-Fall durch **Kapselung** (§2.2–2.4).

```
 Main-Thread (React)                    Regeneration-Worker (ein JS-Realm)
 ─────────────────────                  ─────────────────────────────────────
 FeatureDialog ──Params──▶ feature_calls       Context (RefCell): CsgNode/BoundedCsg
 OuterApiClient ◀async RPC▶  ┌──────────────────+ abgeleitete B-Rep + BranchId ──────┐
 (Preview/Tooling)           │  regenerate()                       [RUST/WASM]       │
                             │  für jeden FeatureCall:                               │
                             │   • Built-in  → execute(&doc); ctx.commit()  (nativ)  │
                             │   • FieldSkript ───────────┐                          │
                             │                            ▼                          │
                             │           js_run_script_feature(scriptId, params, ctx)│
                             │                            │       [Rust→JS sync]     │
                             │   ┌────────────────────────▼────────────────────────┐ │
                             │   │ build(context,id,def)                 [TS/Sand]  │ │
                             │   │  fd.op*() / fd.call() ─┐  fd.q.eval(q) ─┐        │ │
                             │   └───────────────────────┼────────────────┼────────┘ │
                             │   [JS→Rust sync re-entry] │                │          │
                             │     fd_op_commit(ctx,…) ──┘ (borrow_mut)   │          │
                             │     fd_resolve_query(ctx,q) ◀──────────────┘ (borrow) │
                             │       → Handles / EntityRefs (Context wächst in place) │
                             └───────────────────────────────────────────────────────┘
```

### 2.1 Das eine echte Problem: Re-Entrancy & Aliasing

Wenn `regenerate` (mitten in einem WASM-Aufruf) ein Skript-Feature aufruft, läuft Rust **auf
dem Stack**. Ruft dessen `build()` synchron in den Kernel zurück (`fd.q.eval`), ist das ein
**re-entranter WASM-Aufruf, während ein WASM-Aufruf schon läuft**. Auf WASM-Ebene ist das
erlaubt; das einzige reale Risiko ist **Rusts Aliasing-Regel**: hält der äußere Loop ein
`&mut Document`, und der re-entrante Call braucht `&Document`, paniced der `RefCell`-Borrow.

Alle „hässlichen Callbacks" (sync↔async, Rust-Query-/Body-Zugriff, Skript-ruft-Skript)
reduzieren sich auf dieses eine Problem. Es löst sich **nicht** durch „verbiete Mutation"
(das ist FieldSkripts Zweck, §2.2), sondern durch **Zustands-Trennung** (Document immutabel /
Context mutabel) **+ Single-Writer-Borrow-Disziplin** (§2.6): der Loop hält keinen Borrow über
den Callback, jeder Re-Entry borgt kurz und disjunkt.

### 2.2 Was über die Grenze geht: **Context-Mutation, nicht Document-Mutation**

> **Korrektur eines früheren Entwurfsfehlers.** Ein erster Entwurf modellierte `fd.op*` als
> *reine* Konstruktoren (Knoten in eine JS-Arena hängen, nichts mutieren). Das ist **falsch** und
> widerspricht FeatureScripts Kern: Ein FieldSkript ruft vor allem **andere Features (meist
> Built-ins) auf, und die mutieren**. Entscheidend ist das **Query-after-Create-Muster** im
> *selben* `build()`:
> ```fs
> opExtrude(context, id + "boss", {...});                 // erzeugt Körper IM context
> var topFace = qCreatedBy(id + "boss", EntityType.FACE);  // fragt das eben Erzeugte ab
> opFillet(context, id + "fillet", { entities: topFace, radius: r });
> ```
> Hängte `opExtrude` nur ein `NodeRef` in ein JS-Array, gäbe es noch **keine B-Rep**, und
> `qCreatedBy` könnte nicht auflösen. Also **muss** die Mutation über die Grenze. Genau das ist
> FieldSkripts Aufgabe.

Die saubere Auflösung ist **Zustands-Trennung** — exakt Onshapes `context`-Modell:

| Zustand | Mutabilität während `build()` | Wer schreibt |
|---|---|---|
| **Document** (Feature-Liste, Params, Variablen) | **immutabel** — Eingabe | niemand. `build()` schreibt **nie** den Feature-Baum |
| **Context** (entstehende Geometrie: `CsgNode`/`BoundedCsg`, abgeleitete B-Rep, `BranchId`-Provenienz) | **mutabel** — Scratch | `fd.op*` mutiert ihn inkrementell |

`fd.op*` ist also **imperativ**: ein synchroner Call in den Kernel, der die Operation **in den
lebenden Context anwendet** (commit), `BranchId`-Provenienz stempelt und **abfragbare Handles**
zurückgibt. Spätere Calls im selben `build()` sehen die Wirkung. Das ist Onshapes `opExtrude`-
Semantik 1:1.

**Was die Grenze NICHT trägt:** Document-Mutation. `build()` kann den Feature-Baum nicht
umschreiben, keine Params anderer Features ändern, nichts persistieren. **Das** ist die
Invariante, die Determinismus, Caching und Undo trägt — nicht „keine Mutation überhaupt".

> **Die richtige Verzögerung sitzt woanders.** Unser B-Rep ist **abgeleitet** (Octree-Weben aus
> SDF), nicht authored wie Parasolid. „Erzeuge, dann frage sofort Flächen ab" verlangt also einen
> Webe-Schritt zwischen Op und Query. Lösung: **CsgNode/Provenienz eager** pro Op aktualisieren
> (billig), **B-Rep-Weben lazy** — erst materialisieren, wenn eine Query es braucht, und cachen
> (greift in die Weave-Caches von [[brep-local-coords-rework]]). Verzögert wird das *Weben*, nicht
> die *Op-Anwendung*. Das ist der wahre Kern, der „deferral" wertvoll macht — der erste Entwurf
> hatte ihn an der falschen Stelle.

### 2.3 Query-Brücke — read gegen den **lebenden** Context

Queries lesen den Context *während* `build()` — und zwar seinen **aktuellen, durch frühere Ops
dieses Features mitgewachsenen** Stand (nicht einen eingefrorenen Snapshot — sonst bräche
Query-after-Create). Eine synchrone, **read-only** Grenzfunktion:

```rust
#[wasm_bindgen]
pub fn fd_resolve_query(query_ast_json: &str, ctx: ContextHandle) -> String // → EntityRef-Set JSON
```

JS baut mit `fd.q.*` lazy einen **Query-AST**; `fd.q.evaluate(q)` ruft `fd_resolve_query`
synchron und erhält konkrete, stabile `EntityRef`. Die Funktion mutiert nicht (sie kann das
lazy B-Rep-Weben **auslösen**, aber das ist memoisierte Materialisierung, kein Modell-Change).

### 2.4 Sub-Feature-Trampolin — Skript ruft Skript / Built-in

FieldSkripts Hauptzweck. Feature-Ausführung commitet in den Context, ein Sub-Aufruf ist daher:

- **Skript ruft Skript** (Komposition): `fd.call(scriptType, params)` schlägt das Ziel in der
  Worker-`Map<scriptId, BuildFn>` nach und ruft dessen `build()` **rekursiv in reinem JS** auf —
  mit **demselben `context`-Handle**, sodass alle Ops in denselben lebenden Context committen.
  **Kein Grenzübertritt** für die Orchestrierung; nur die `op*`/Query-Calls darin queren wie üblich.
- **Skript ruft Built-in** (95-%-Bausteine, `fd.call("box", …)`): Die Built-in-Logik ist die
  reine Funktion `params → CsgNode`. Zwei Schichten:
  ```rust
  pub fn fd_eval_builtin(type_name: &str, params_json: &str) -> String  // PURE: → CsgNode-Subtree
  pub fn fd_op_commit(ctx: ContextHandle, subtree_json: &str, /* op,blend,targets */) -> String // MUTIERT: → Handles
  ```
  `fd.call`/`fd.op*` = `eval_builtin` (pure) **+** `op_commit` (mutiert, webt provenienz, gibt
  Query-Handles). Damit bleibt die **Geometrie-Logik der Built-ins ein einziger, reiner Pfad** —
  egal ob der Rust-Loop sie direkt aufruft (95-%-Pfad) oder ein Skript via `fd.call`.

**Zyklus-Schutz:** Aufruf-Stack-Set erkennt Rekursion (A→B→A), Tiefenlimit kappt Endlos-
Komposition — deterministisch.

### 2.5 Die ganze Grenzfläche

Vier schmale, synchrone Funktionen; die Mutation ist auf **eine** Funktion konzentriert, die nur
den **Context** (nie das Document) anfasst:

| Richtung | Funktion | Effekt |
|---|---|---|
| Rust → JS (1 Import) | `js_run_script_feature(scriptId, paramsJson, ctx) → ()` | führt 1 `build()` aus |
| JS → Rust | `fd_resolve_query(ast, ctx) → entityRefs` | **read-only** (löst ggf. lazy Weave aus) |
| JS → Rust | `fd_eval_builtin(typeName, paramsJson) → subtree` | **pure** (kein State) |
| JS → Rust | `fd_op_commit(ctx, subtree, op…) → handles` | **mutiert Context** (nie Document) |

Op-Builder, Skript-zu-Skript-Rekursion und Param-Dekodierung bleiben **reines JS bzw. reines
Rust je auf seiner Seite**. Der `ctx`-Handle identifiziert den lebenden Context (statt eines
„Frozen-Epoch"-Tokens — der Context ist absichtlich *nicht* eingefroren). Die bidirektionale
Fläche kollabiert damit auf **4 Funktionen mit genau einem klar markierten Mutator** — kein async
im heißen Pfad, Document-Immutabilität intakt.

### 2.6 Re-Entrancy & Borrow-Disziplin — der reale Rust-Umbau

Weil `fd_op_commit` den Context **mutiert** und re-entrant aufgerufen wird, während `regenerate`
auf dem Stack liegt, ist die Soundness-Invariante **nicht** „Frozen Snapshot", sondern
**Single-Writer durch nicht-überlappende Borrows** (alles single-threaded & synchron):

- Der **Context lebt hinter `RefCell`/Handle**, *außerhalb* des per-Feature-Borrows des Loops.
- Der äußere Loop hält **keinen** Borrow über `js_run_script_feature`.
- Jeder Re-Entry nimmt einen **kurzlebigen** Borrow: `borrow_mut()` für `fd_op_commit`,
  `borrow()` für `fd_resolve_query` — mutiert/liest, **gibt sofort frei**.
- Da synchron & single-threaded, sind diese Borrows **zeitlich disjunkt** → `RefCell` ist happy,
  kein Aliasing. Das **Document** bleibt durchweg immutabel (separater Borrow, nie über den
  Callback gehalten).

```
für jedes Feature N:                        // Context = RefCell, NICHT vom Loop geborgt
  match kind {
    Rust(f)   => { let s = f.execute(&doc); ctx.borrow_mut().commit(s, …); } // 95 %: nativ, kein JS
    Script(s) => js_run_script_feature(s, params, ctx_handle)               // Callback:
        // └─ darin re-entrant: fd_op_commit → ctx.borrow_mut() (kurz) ; fd_resolve_query → ctx.borrow()
  }
```

Falls `regenerate` heute `&mut self` über alles hält, ist der Umbau: **Context aus dem
Loop-Borrow herauslösen** (eigenes `RefCell`/Owner), Loop hält ihn nur noch per Handle. Real,
aber begrenzt — und **DL-61 schiebt in dieselbe Richtung** (Feature emittiert Subtree → committen).

### 2.7 Warum A das 95/99-Profil bedient

- **Caching:** Skript-Cache-Key = `hash(scriptId, compiledJsHash, paramsJson, aufgelöste
  Query-Ergebnisse)`. Unverändert ⇒ committeten Context-Zuwachs wiederverwenden, **JS-Callback
  überspringen**. Auch der 1-%-Skript-Pfad ist damit billig.
- **Performance:** Built-ins im 95-%-Pfad queren die Grenze **nie** (Rust-Loop ruft `execute` +
  `commit` direkt, in Rust). Nur echte Skript-Evaluationen zahlen pro Op/Query einen Re-Entry —
  und dort dominiert ohnehin das B-Rep-Weben, nicht der FFI-Hop. (Mehr Grenzverkehr als der
  verworfene Deferred-Entwurf, aber auf dem seltenen Pfad und korrektheits-notwendig.)
- **Determinismus:** synchron + Document-immutabel + seeded RNG + kein fetch/Timer ⇒ `build()` ist
  reine Funktion von `(Document, Context-vorher)`. Mutiert wird nur Scratch-Geometrie, reproduzierbar.
- **Headless/Server:** Reine Rust-Dokumente regenerieren **ganz ohne JS-Runtime**; nur Dokumente
  mit Skript-Feature brauchen die JS-Engine — opt-in statt global (anders als bei B).

> **Synchron, nicht über Messages.** Der heiße Pfad ist **kein** `postMessage`/RPC: Rust→JS
> (`js_run_script_feature`) und JS→Rust (`fd_op_commit`/`fd_resolve_query`) sind **synchrone
> wasm-bindgen-Calls im selben Worker-Realm**. `postMessage` (async) existiert nur am **äußeren**
> Rand Main-Thread ↔ Worker (`OuterApiClient`, Preview/Tooling). Async im heißen Pfad würde
> Query-after-Create (§2.2) brechen.

**Bezug DL-61:** Sowohl der direkte Built-in-Pfad als auch `fd_eval_builtin` liefern denselben
**offenen `CsgNode`-Subtree**; `fd_op_commit` hängt ihn mit Provenienz in den Context. FieldSkript
ist damit der **laufzeit-pluggbare Orchestrator** über derselben IR wie die Built-ins — der
Unterschied ist nur: ein Built-in ist *eine* pure `execute`, ein FieldSkript *verkettet* viele,
mit Queries dazwischen.

---

## 3. Sprache & Sandbox

FieldSkript läuft als **JavaScript im Worker-Realm neben dem Kernel-WASM** — derselbe JS-Realm,
der schon `globalThis.fd` injiziert (§4.6). Der Browser-eigene JIT (V8/SpiderMonkey) ist die
Engine; **kein eingebetteter Interpreter** (verworfen — der Kernel bleibt Hoheit + Loop, TS dockt
nur als Callback über die wasm-bindgen-Grenze an, §2).

- **Quellsprache: TypeScript.** Pro Skript transpiliert nach JS (esbuild-wasm oder
  sucrase — beide browserfähig, sucrase ist schneller/typprüfungsfrei, esbuild liefert
  bessere Fehlerpositionen). Transpilation passiert **beim Speichern** (CRUD), nicht im
  heißen Regen-Pfad; gespeichert wird `{ source, compiledJs, schema }`.
- **Ausführung: bestehende Determinismus-Sandbox** (`workerSandbox.ts`). `freezeGlobals`
  + `reseedRandom(seed = hashDocumentJson(...))` gelten unverändert; FieldSkript erbt sie.
  `new Function(compiledJs)` im gefrorenen Realm. Skripte sehen **nur** `fd` + reine Intrinsics
  (`Math`, `Array`, …), **kein** DOM/Netz/FS.
- **Härtegrad — gestaffelt:**
  - *Jetzt (eigene/vertraute Skripte):* Soft-Sandbox wie heute (Determinismus, nicht
    Sicherheit — wie README §11 Risiko 5 dokumentiert).
  - *Später (geteilte Bibliotheks-Skripte):* **QuickJS-emscripten** als härterer,
    ressourcenlimitierter JS-Realm **in JS-Land** (synchron, deterministisch) — eine
    *Sandbox-Härtung*, kein Wechsel der Treiber-Hoheit (Rust treibt weiterhin). Erst nötig,
    wenn Skripte zwischen Usern geteilt werden.
- **Reinheit (cache-kritisch):** Ein `build()` muss **pure** sein (gleicher `(Document,
  Context-vorher)` → gleiche Knoten). Das ist die Voraussetzung für den BranchId-Cache, Undo
  und inkrementelle Regeneration. Instruktions-/Zeitbudget pro Lauf kappt Endlosschleifen.

---

## 4. Das Feature-Kontrakt

Ein FieldSkript-Modul exportiert **zwei** Dinge: die **Definition** (Meta) und die
**Build-Funktion** (Geometrie). TS-Form:

```ts
import { fd, length, bool, enumParam, query, group, visibleWhen,
         EntityType, GeometryType, Context, Id, Def } from "fielddraft";

// (1) META — deklarativ, serialisierbar → erzeugt das Schema-JSON,
//     das FeatureDialog schon generisch rendert.
export const definition = fd.feature({
  name: "Counterbore Hole",
  nameTemplate: "Counterbore (#diameter)",
  parameters: [
    query("faces", { name: "Placement faces",
                     filter: EntityType.FACE.and(GeometryType.PLANE),
                     maxPicks: 1 }),
    length("diameter", { name: "Diameter", min: 0, default: 8, max: 1000, unit: "mm" }),
    length("depth",    { name: "Depth",    min: 0, default: 20, unit: "mm" }),
    bool("flip", { name: "Flip", uiHint: "OPPOSITE_DIRECTION" }),
    group("Counterbore", { collapsed: false }, [
      length("cbDiameter", { name: "C'bore Ø", default: 14, unit: "mm" }),
      length("cbDepth",    { name: "C'bore depth", default: 5, unit: "mm" }),
    ]),
  ],
});

// (2) BUILD — Geometrie-Emission über fd.op* (DL-61-Knoten).
export default function build(context: Context, id: Id, def: Def) {
  for (const face of fd.query.evaluate(context, def.faces)) {
    const axis = fd.face.normal(context, face);
    fd.opHole(context, id.sub("bore"), {
      at: face, axis, diameter: def.diameter, depth: def.depth, flip: def.flip,
    });
    fd.opHole(context, id.sub("cbore"), {
      at: face, axis, diameter: def.cbDiameter, depth: def.cbDepth, flip: def.flip,
    });
  }
}
```

`definition` und `build` ersetzen Onshapes `precondition {}` + Body, bleiben aber **gewöhnliches
TS** (keine Spezialsyntax) und damit Monaco-/tsc-prüfbar.

---

## 5. Meta-Auszeichnung der Parameter

Die **deklarative DSL** (Builder-Funktionen oben) ist der Laufzeit-Zwilling des
`#[derive(FeatureSchema)]`-Macros. Sie wird beim Speichern **einmal ausgeführt** und
emittiert **exakt das Schema-JSON**, das `schema.ts`/`FeatureDialog` bereits konsumieren —
d.h. **null neue UI-Arbeit** für skriptbare Features.

### Onshape-Annotation → FieldSkript-DSL → bestehendes Schema-JSON

| Onshape | FieldSkript-DSL | Schema-`type` | UI heute |
|---|---|---|---|
| `isLength(d.x, LENGTH_BOUNDS)` | `length("x",{min,default,max,unit})` | `length` | Zahlfeld + Einheit |
| `isAngle(d.a, ANGLE_BOUNDS)` | `angle("a",{…})` | `angle` | Winkelfeld |
| `isReal(d.r, REAL_BOUNDS)` | `real("r",{min,default,max})` + `uiHint:"slider"` | `f32` | Feld/Slider |
| `d.b is boolean` | `bool("b",{default,uiHint})` | `bool` | Checkbox / ±-Flip |
| `d.s is string` | `str("s",{maxLength})` | `string` | Textfeld |
| `d.e is MyEnum` | `enumParam("e", MyEnum,{uiHint:"HORIZONTAL_ENUM"})` | `enum` (+`variants`) | Tabs / Dropdown |
| `d.q is Query` + `"Filter"` | `query("q",{filter, maxPicks})` | `query` | Selektion |
| `if (d.useX) { … }` | `visibleWhen("useX", …)` Wrapper | `visible_when` | bedingt sichtbar |
| `annotation {"Group Name"}` | `group("Name",{collapsed,driving},[…])` | `children`-Gruppe | Collapsible |
| `definition.items is array` | `arrayParam("items", itemSchema,{itemLabel})` | `array` | Wiederhol-Liste |

**UIHints** (1:1 zur bestehenden Vokabel in `features-macros`): `HORIZONTAL_ENUM`,
`OPPOSITE_DIRECTION`, `slider`, `REMEMBER_PREVIOUS_VALUE`, `ALWAYS_HIDDEN`,
`NO_PREVIEW_PROVIDED`, `COLLAPSE_ARRAY_ITEMS`.

**Bounds** = `LengthBoundSpec`-Analog: `{min, default, max, unit}` → Schema-Felder
`min`/`max`/`step`/`defaultValue`/`unit` (teils schon emittiert, Rest aus dem §4.6-Vokabular,
das `schema.ts` bereits kennt: `visible_when`, `children`, `min/max`).

**Sichtbarkeit / Preconditions:** Onshapes `if`-gegatete Parameter bilden wir über
`visibleWhen(predicate, params)` ab; `predicate` ist auf Vergleiche vorheriger Bool/Enum-Params
beschränkt (`==`,`!=`,`&&`,`||`,`!`) — exakt die `visible_when`-Semantik, die das Schema schon trägt.
So bleibt die Auswertung **statisch im Dialog** (kein Skriptlauf fürs Dialog-Layout).

> **Warum deklarativ statt TS-Decorators?** Die Param-Deklaration muss **ohne** Ausführung des
> `build()`-Bodys und **serialisierbar** sein (das Schema wird gespeichert und im Insert-Menü
> gebraucht). Ein reines Daten-Objekt (`fd.feature({parameters:[…]})`) ist serialisierbar,
> tsc-typbar (die Builder geben typisierte `Param<T>` zurück, aus denen sich der `Def`-Typ
> ableiten lässt) und braucht keine Decorator-/Reflect-Metadata-Toolchain.

---

## 6. Das Query-System für Körper & B-Rep

Das Herzstück und der größte Hebel: FieldDraft hat mit **`BranchId`-Provenienz + stabilen
FNV-1a-Topologie-IDs** (DL-31…DL-60) bereits das Substrat, an dem das
**Topological-Naming-Problem** gelöst wird. Das Query-System macht dieses Substrat für
Skripte adressierbar.

### Ist-Stand (`queries.rs`, K4.5.7)
`QueryExpr {CreatedBy(id), FaceOf(id), EdgeBetween(a,b)}` + Parser + **Feature-Ebenen**-Evaluator
+ versions-keyed Cache. **Lücken:** nicht in `Context` verdrahtet; löst auf
Contributor-`FeatureId`-Sets auf, **nicht** auf konkrete B-Rep-Elemente (Face/Edge/Vertex);
keine Mengen-/Geometrie-/Adjazenz-Operatoren.

### Ziel-Vokabular (gestaffelt aufzubauen)

```
EntityType   = VERTEX | EDGE | FACE | BODY
GeometryType = PLANE | CYLINDER | SPHERE | CONE | TORUS | LINE | CIRCLE | ARC | NURBS
BodyType     = SOLID | SURFACE | WIRE | POINT
```

| Klasse | Onshape | FieldSkript `fd.q.*` | Auflösung gegen |
|---|---|---|---|
| **Provenienz** | `qCreatedBy`, `qOwnerBody` | `createdBy(id, EntityType)`, `ownerBody(q)` | `BranchId`-Index (existiert) |
| **Universum** | `qEverything`, `qNothing` | `everything(EntityType)`, `nothing()` | B-Rep-Element-Tabelle |
| **Mengen** | `qUnion`, `qIntersection`, `qSubtraction` | `union/intersect/subtract(…)` | ordnungserhaltend (Onshape-Garantie) |
| **Geometrie-Filter** | `qGeometry`, `qBodyType`, `qParallelPlanes` | `geometry(q, FACE, PLANE)`, `bodyType(q, SOLID)` | analytische Face-Typen (DL-50/57) |
| **Räumlich** | `qContainsPoint`, `qNearestTo`, `qLargest` | `containsPoint(q,p)`, `nearestTo(q,p)`, `largest(q)` | B-Rep-Geometrie (Fläche/Länge) |
| **Topologische Adjazenz** | `qAdjacent`, `qEdgesOf`, `qVertexAdjacentTo`, `qLoops` | `adjacent(q, EDGE)`, `edgesOf(face)`, `loopsOf(face)` | Half-Edge/Weave-Topologie (DL-58) |
| **Stabile Referenz** | (Onshape intern) | `byStableId(fnvId)` | FNV-1a-Naming (DL-31) |

### Semantik & Auflösung
- **Lazy & regenerationsfest:** Ein Query wird als **AST** in `params` serialisiert (nicht als
  aufgelöste IDs), und **bei jeder Regeneration neu** gegen den aktuellen Context ausgewertet.
  Bewegt sich Upstream-Geometrie, zeigt der Fillet weiterhin auf die geometrisch korrekte Kante —
  genau Onshapes Versprechen, hier aber über die FNV-1a-Signaturen abgesichert.
- **`evaluateQuery(context, expr) → [EntityRef]`** wird von der **Feature-Ebene auf die
  B-Rep-Element-Ebene** gehoben: `QueryResult` trägt künftig konkrete `EntityRef`
  (`{kind: Face|Edge|Vertex, branchSignature, stableId}`) statt nur Contributor-Sets. Das ist
  der in `queries.rs` bereits angekündigte „proxy element handle"-Ausbau.
- **Context-Integration:** `evaluateQuery` bekommt Zugriff auf den **lebenden** Context
  (in Aufbau befindliche B-Rep), damit ein Feature Entitäten abfragen kann, die frühere
  Features im selben Lauf erzeugt haben (Onshapes „created by an earlier operation").
- **Query als Parameter:** Der `query("faces",{filter})`-Param liefert dem Skript einen
  `Query`-Wert; der `filter` (EntityType/GeometryType-Kombination) begrenzt **die Picks im
  Viewport** und wird im Schema-JSON als `filter` mitgegeben (Selektions-Gating in der UI).
- **Cache:** Der vorhandene versions-keyed `QueryCache` greift unverändert; B-Rep-Element-Ebene
  erweitert nur den Result-Typ, nicht die Invalidierungslogik.

---

## 7. Operationen (`fd.op*`) — Geometrie-Emission

Die Op-Vokabel ist die TS-Brücke zu den Rust-`FeatureKind`-Bausteinen und emittiert
DL-61-Knoten:

- **Primitive:** `opBox/opSphere/opCylinder/opCone/opTorus/…` → `CsgNode::Leaf(Primitive)`.
- **Sketch-getrieben:** `opExtrude/opRevolve/opSweep(profileQuery|sketch, …)`.
- **Modifizierend:** `opFillet/opChamfer(edgeQuery, r)`, `opShell(faceQuery, t)`,
  `opTwist/opBend/opDisplace` → die schon existierenden `CsgNode::Twist/Bend/Displace/Shell/SmoothUnion`-Wrapper.
- **Boolesch:** `opBoolean(target, tool, UNION|DIFFERENCE|INTERSECT, {blend})`.
- **Muster/Spiegel:** `opLinearPattern/opCircularPattern/opMirror(query, …)`.

**ID-System:** `id.sub("name")` erzeugt hierarchische `FeatureId`s (Onshapes `id + "fillet1"`).
Jede Op stempelt ihre Knoten mit `BranchId::from_feature_sub(id, …)` → stabile Provenienz für
Naming **und** Queries. Die hierarchische `FeatureId`-Form ist im Kernel bereits vorgesehen
(`queries.rs` nennt `qCreatedBy(extrude_3.op.mask_0)` als geplante Tiefe).

---

## 8. Persistenz, CRUD & Scope

### Datenmodell
```ts
interface FieldSkriptRecord {
  id: string;               // stabile Skript-Typ-ID (= FeatureCall.type_name)
  name: string;
  source: string;           // TypeScript-Quelle (Editor)
  compiledJs: string;       // transpiliert beim Speichern
  schema: RawFeatureSchema; // aus definition extrahiert → FeatureDialog
  version: number;          // bump bei Quelländerung
}
```

### Scope (gestaffelt)
1. **Dokument-lokal zuerst** (analog Onshapes „Feature Studio im selben Document"): Skripte
   leben in `Document` (neben `feature_calls`). Selbstständig, kein Backend, deckt CLAUDE.md
   („Backend optional"). Ein FieldSkript-Feature ist ein normaler `FeatureCall` mit
   `type_name = record.id` und `params` = User-Werte.
2. **Geteilte Bibliothek später**: versionierte Importe über das Backend (`server/`), dann mit
   QuickJS-Härtung.

### Lifecycle
```
Editor speichern
  → transpile(source) → compiledJs
  → run(definition)   → schema          (im Sandbox-Realm, ohne build())
  → store FieldSkriptRecord
  → Registry: type_name wird im Insert-Menü/Toolbar wie ein Built-in sichtbar
Insert
  → FeatureDialog(schema)  → params
  → push_feature_call(type_name, params)   (bestehender Pfad!)
  → Regeneration: Treiber findet Skript-Typ → run(build, ctx, id, def) → Knoten
```

Der gesamte **Insert-/Dialog-/Regenerations-Pfad existiert bereits** — FieldSkript hängt sich
über `type_name` + Schema-JSON ein, ohne Sonderbehandlung in der UI.

---

## 9. Editor

**Empfehlung: Monaco-Editor mit eingebautem TypeScript-Language-Service** — *nicht* eine
handgeschriebene Monarch-Grammatik.

- **Monarch** ist nur Monacos deklarative **Tokenizer-/Syntax-Highlight**-Sprache. Für eine
  *eigene* DSL bräuchte man sie. FieldSkript **ist** TypeScript → Monaco bringt den vollen
  TS-Worker mit: **Autovervollständigung, Hover-Typen, Live-Diagnostics, Go-to-Definition**.
  Das ist strikt mächtiger als Highlighting.
- **`fielddraft.d.ts` als `extraLib`:** Wir füttern Monacos TS-Worker mit den Ambient-Typen der
  gesamten `fd.*`-API (Ops, Query-Builder, Param-Builder, `Context`/`Id`/`Def`). Damit bekommt
  der User **IntelliSense auf das echte API** und Fehler **vor** dem Speichern.
- **Typisierte `Def`:** Aus den `parameters`-Buildern lässt sich (über TS-Inferenz) der
  `Def`-Typ ableiten, sodass `def.diameter` als `Length` typisiert ist — Tippfehler bei
  Param-Namen werden zu Compile-Fehlern.
- **Live-Preview:** Editor-Save → transpile → Outer-API-Preview-Lauf (async) → Viewport-Update;
  Fehler (Transpile/Runtime) erscheinen als Marker im Editor und in einem Diagnostics-Panel.
- Monarch nur als **Fallback** sinnvoll, falls man später eine bewusst eingeschränkte
  Nicht-TS-Sprache anbietet (derzeit nicht geplant).

CRUD-Oberfläche: Listen-/Detailpanel (erstellen, umbenennen, duplizieren, löschen, Versionen),
Editor im Modal/Side-Panel, „Insert"-Button registriert den Typ. Param-Header-Vorbild und
Skript-Verwaltung sind aus `fieldscript.ts` (P2.8) bereits prototypisch da und werden
verallgemeinert.

---

## 10. Sicherheit & Determinismus

- **Determinismus** ist Vertrag (Cache, Undo, bit-identische Regeneration): Sandbox aus
  `workerSandbox.ts` (kein `fetch`/Timer, `Date.now→0`, mulberry32-`Math.random`). `build()`
  muss pure sein.
- **Ressourcenbudget:** Instruktions-/Zeit-Cap pro `build()`-Lauf (Endlosschleifen-Schutz),
  später hart via QuickJS.
- **Isolation:** Soft jetzt (README §11 Risiko 5 dokumentiert die Grenze), QuickJS-VM für
  geteilte Skripte. Skripte sehen **nur** `fd` + reine Standard-Intrinsics, **kein** DOM,
  Netzwerk, Dateisystem.

---

## 11. Inkrementeller Umsetzungsplan

| Stufe | Inhalt | Baut auf |
|---|---|---|
| **S0** | DL-61 Stufe A/B: `Feature`→`DecodedFeature{node}`, Ops emittieren Knoten | DL-61 |
| **S1** | `fielddraft.d.ts` + Param-DSL (`fd.feature`, `length/bool/enum/query/group`) → Schema-JSON; FieldSkript-Record + dokument-lokale Persistenz | `schema.ts`, FeatureDialog |
| **S2** | Rust-Treiber-Callback (§2): `js_run_script_feature`-Import; Context aus dem Loop-Borrow lösen (`RefCell`/Handle, §2.6); `fd_op_commit` (mutiert Context) + `fd_eval_builtin` (pure) | §4.6 Dual-API, K4.5.8 |
| **S2b** | Query-Brücke `fd_resolve_query` gegen lebenden Context (read-only, lazy Weave); Sub-Feature-Trampolin (`fd.call`: Skript→Skript JS-Rekursion, Skript→Built-in via eval+commit) | S2, `queries.rs` |
| **S3** | Query-Ausbau: `evaluateQuery` Feature- → B-Rep-Element-Ebene; `createdBy/geometry/bodyType/adjacent/nearestTo`; Context-Integration | `queries.rs`, Naming DL-31… |
| **S4** | Monaco-Editor + TS-Service + `extraLib`; CRUD-Panel; Live-Preview | `fieldscript.ts`-Vorbild |
| **S5** | Härtung (QuickJS), geteilte Bibliothek über Backend, Versionierung | `server/` |

---

## 12. Offene Entscheidungen

1. ~~**Treiber-Inversion**~~ **ENTSCHIEDEN (§2.0):** Rust treibt, FieldSkript ist Callback.
   Begründung 95 % native Ausführungen / 99 % Skript-Typen. `fd.op*` mutiert den **Context**
   (nicht das Document) — Soundness über Zustands-Trennung + Single-Writer-Borrows (§2.2/2.6),
   nicht über Mutationsverbot.
6. **Weave-Timing:** B-Rep lazy-on-query weben (§2.2) — pro Op, pro Query, oder gebatcht? Messen,
   sobald Query-after-Create-Skripte real sind.
2. ~~**Built-ins doppeln?**~~ **ENTSCHIEDEN (2026-06-01, User): NEIN.** Built-ins bleiben in Rust
   und werden **nicht** als FieldSkript reimplementiert. FieldSkript ist für Komposit-/Custom-
   Features; die 25 nativen Built-ins bleiben der schnelle Rust-Pfad (`FeatureKind::execute`).
   Kein Dogfooding-Zwang, kein zweiter Pfad für dieselbe Geometrie.
3. **Query-Tiefe:** Wie weit treiben wir Adjazenz/räumliche Queries in S3 vs. später?
4. **Transpiler:** esbuild-wasm (Fehlerqualität) vs. sucrase (Geschwindigkeit) — messen.
5. **`Def`-Typinferenz:** Wie viel TS-Typmagie aus `parameters` ableiten, ohne den Editor zu
   verlangsamen?
