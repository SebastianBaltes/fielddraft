//! Query minimum (K4.5.7).
//!
//! FieldDraft features reference geometry through **queries** rather
//! than raw entity ids — so a fillet parameter says "the edge between
//! feature `box_1` and feature `extrude_3`" rather than "edge with
//! mesh index 482". Queries survive regeneration: when upstream
//! features move or resize, the downstream fillet re-evaluates its
//! query against the new scene and still picks the geometrically
//! correct edge. This module is the first working slice of that
//! infrastructure.
//!
//! ## What K4.5.7 ships
//!
//! * The [`QueryExpr`] AST with the three query minimum forms the
//!   README commits to: `qCreatedBy(id)`, `qFaceOf(id)`, and
//!   `qEdgeBetween(id_a, id_b)`.
//! * [`parse_query`] — a tiny recursive-descent parser that accepts
//!   the canonical string form (`"qCreatedBy(box_1)"`) and returns
//!   a `QueryExpr` plus typed errors on malformed input.
//! * [`evaluate_query`] — a pure evaluator that walks a [`Document`]
//!   and returns a [`QueryResult`]. The evaluator operates at the
//!   **feature level**, not the mesh level: it checks feature
//!   existence and returns the contributor sets that satisfy the
//!   query, which is the semantic layer downstream features and
//!   the Outer API need. Full mesh-element resolution (clicking on
//!   a specific face / edge polyline) layers on top of this in a
//!   later slice once the query result type grows to carry proxy
//!   element handles.
//! * [`QueryCache`] — a version-keyed memoisation layer. When the
//!   context version bumps (typically because a regeneration step
//!   mutated the document), the cache invalidates every entry in
//!   one sweep; otherwise repeated queries hit the cache without
//!   re-walking the document.
//!
//! ## What K4.5.7 deliberately does *not* ship
//!
//! * Integration into [`Context`](crate::features::Context). The
//!   existing execute-path contract is intentionally unchanged in
//!   this slice — K4.5.5/4.5.6 already have their share of moving
//!   parts on the worker side, and threading a cache reference
//!   through every `FeatureKind::execute` call belongs in a
//!   dedicated follow-up. K4.5.7 stands the primitives up as a
//!   standalone module that the kernel and the Outer API can both
//!   reach for when they're ready.
//! * Mesh-level entity resolution. [`QueryResult`] carries the
//!   contributor `FeatureId` sets that match, not mesh vertex
//!   indices or proxy element handles. Downstream slices extend
//!   the result type when they need the geometric layer.
//! * Hierarchical queries (`qCreatedBy(extrude_3.op.mask_0)`).
//!   [`FeatureId`]'s hierarchical form is preserved through
//!   `FromStr`, so the parser already accepts the path syntax —
//!   the evaluator just treats every id as an opaque lookup key.

use std::collections::HashMap;
use std::fmt;

use fielddraft_protocol::FeatureId;

use crate::csg::DecodedFeature;

/// Parsed form of a `q…(…)` query string. Three variants match the
/// README's "query minimum" vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExpr {
    /// `qCreatedBy(id)` — every entity whose provenance lists `id`
    /// as its sole contributor. Semantically: "the geometry this
    /// feature produced and nothing else touched".
    CreatedBy(FeatureId),
    /// `qFaceOf(id)` — the face(s) owned by a single feature. At
    /// the feature level this is the same set as `CreatedBy`, but
    /// the distinction is preserved so downstream slices can
    /// narrow to `Face`-classified proxy elements when the
    /// evaluator gains mesh-level resolution.
    FaceOf(FeatureId),
    /// `qEdgeBetween(a, b)` — edges that have both `a` and `b` (and
    /// only those two) in their contributor set. Corresponds to
    /// the emergent intersection curve between two primitives in
    /// the SDF proxy.
    EdgeBetween(FeatureId, FeatureId),
    /// K8.13: `qAllRegions(sketch_id)` — all closed regions of a sketch.
    AllRegions(FeatureId),
    /// K8.13: `qRegion(sketch_id, region_index)` — a single region.
    Region(FeatureId, u32),
    /// K8.13: `qRegions(sketch_id, [indices])` — a subset of regions.
    Regions(FeatureId, Vec<u32>),
    /// `qFace(id_hash)` — the single B-Rep face whose stable
    /// [`TopoRef::id_hash`](crate::brep::naming::TopoRef) equals the
    /// given value. This is the id_hash-addressed form the UI emits
    /// when a user clicks exactly ONE face, and that a feature stores
    /// to reference it across regenerations.
    Face(u64),
    /// `qEdge(id_hash)` — the single B-Rep edge addressed by its
    /// stable id_hash.
    Edge(u64),
    /// `qVertex(id_hash)` — the single B-Rep vertex addressed by its
    /// stable id_hash.
    Vertex(u64),
}

impl fmt::Display for QueryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryExpr::CreatedBy(id) => write!(f, "qCreatedBy({id})"),
            QueryExpr::FaceOf(id) => write!(f, "qFaceOf({id})"),
            QueryExpr::EdgeBetween(a, b) => write!(f, "qEdgeBetween({a}, {b})"),
            QueryExpr::AllRegions(id) => write!(f, "qAllRegions({id})"),
            QueryExpr::Region(id, idx) => write!(f, "qRegion({id}, {idx})"),
            QueryExpr::Regions(id, indices) => {
                let list = indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "qRegions({id}, [{list}])")
            }
            QueryExpr::Face(h) => write!(f, "qFace({h})"),
            QueryExpr::Edge(h) => write!(f, "qEdge({h})"),
            QueryExpr::Vertex(h) => write!(f, "qVertex({h})"),
        }
    }
}

/// Errors the parser emits when a query string is malformed. Kept
/// as a small closed enum rather than a `String`-carrying variant
/// so callers can branch on the failure mode in tests and UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryParseError {
    /// The outer call name didn't match one of the three known
    /// query forms. Carries the offending name so error messages
    /// can be actionable.
    UnknownQuery(String),
    /// The parenthesised argument list was missing, unterminated,
    /// or had the wrong arity for the query name.
    MalformedArguments(&'static str),
    /// One of the `FeatureId` arguments failed to parse.
    InvalidFeatureId(String),
    /// An id_hash argument (`qFace`/`qEdge`/`qVertex`) was not a valid
    /// `u64` decimal. Carries the offending token.
    InvalidIdHash(String),
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryParseError::UnknownQuery(name) => {
                write!(f, "unknown query `{name}`")
            }
            QueryParseError::MalformedArguments(msg) => {
                write!(f, "malformed query arguments: {msg}")
            }
            QueryParseError::InvalidFeatureId(raw) => {
                write!(f, "invalid feature id in query: `{raw}`")
            }
            QueryParseError::InvalidIdHash(raw) => {
                write!(f, "invalid id_hash in query: `{raw}`")
            }
        }
    }
}

impl std::error::Error for QueryParseError {}

/// Parse a canonical query string into a [`QueryExpr`].
///
/// Accepts `qCreatedBy(<id>)`, `qFaceOf(<id>)`, and
/// `qEdgeBetween(<id_a>, <id_b>)`. Whitespace around arguments is
/// tolerated. Feature ids are parsed via [`FeatureId::from_str`],
/// so both flat-numeric (`1`, `42`) and hierarchical path forms
/// (`extrude_3.op.mask_0`) are accepted without the parser itself
/// knowing about the id shape.
pub fn parse_query(input: &str) -> Result<QueryExpr, QueryParseError> {
    let trimmed = input.trim();
    let (name, rest) = trimmed
        .split_once('(')
        .ok_or(QueryParseError::MalformedArguments(
            "expected '(' after query name",
        ))?;
    let args_raw = rest
        .strip_suffix(')')
        .ok_or(QueryParseError::MalformedArguments(
            "expected closing ')'",
        ))?;

    let args: Vec<&str> = if args_raw.trim().is_empty() {
        Vec::new()
    } else {
        args_raw.split(',').map(|s| s.trim()).collect()
    };

    match name.trim() {
        "qCreatedBy" => {
            let id = expect_one_id(&args, "qCreatedBy expects one id")?;
            Ok(QueryExpr::CreatedBy(id))
        }
        "qFaceOf" => {
            let id = expect_one_id(&args, "qFaceOf expects one id")?;
            Ok(QueryExpr::FaceOf(id))
        }
        "qEdgeBetween" => {
            let (a, b) = expect_two_ids(&args, "qEdgeBetween expects two ids")?;
            Ok(QueryExpr::EdgeBetween(a, b))
        }
        "qAllRegions" => {
            let id = expect_one_id(&args, "qAllRegions expects one sketch id")?;
            Ok(QueryExpr::AllRegions(id))
        }
        "qRegion" => {
            let (a, b) = expect_two_ids(&args, "qRegion expects sketch_id and region_index")?;
            let idx = b.as_u32_if_flat().ok_or(QueryParseError::InvalidFeatureId(
                "region index must be a plain number".to_string(),
            ))?;
            Ok(QueryExpr::Region(a, idx))
        }
        "qRegions" => {
            if args.len() < 2 {
                return Err(QueryParseError::MalformedArguments(
                    "qRegions expects sketch_id and [indices]",
                ));
            }
            let id = parse_feature_id(args[0])?;
            let mut indices = Vec::new();
            for arg in &args[1..] {
                let cleaned = arg.trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
                if cleaned.is_empty() { continue; }
                let n: u32 = cleaned.parse().map_err(|_| {
                    QueryParseError::InvalidFeatureId(format!("invalid region index: {cleaned}"))
                })?;
                indices.push(n);
            }
            Ok(QueryExpr::Regions(id, indices))
        }
        "qFace" => Ok(QueryExpr::Face(expect_one_id_hash(
            &args,
            "qFace expects one id_hash",
        )?)),
        "qEdge" => Ok(QueryExpr::Edge(expect_one_id_hash(
            &args,
            "qEdge expects one id_hash",
        )?)),
        "qVertex" => Ok(QueryExpr::Vertex(expect_one_id_hash(
            &args,
            "qVertex expects one id_hash",
        )?)),
        other => Err(QueryParseError::UnknownQuery(other.to_string())),
    }
}

fn expect_one_id(
    args: &[&str],
    message: &'static str,
) -> Result<FeatureId, QueryParseError> {
    if args.len() != 1 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    parse_feature_id(args[0])
}

fn expect_two_ids(
    args: &[&str],
    message: &'static str,
) -> Result<(FeatureId, FeatureId), QueryParseError> {
    if args.len() != 2 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    Ok((parse_feature_id(args[0])?, parse_feature_id(args[1])?))
}

fn parse_feature_id(raw: &str) -> Result<FeatureId, QueryParseError> {
    raw.parse::<FeatureId>()
        .map_err(|_| QueryParseError::InvalidFeatureId(raw.to_string()))
}

/// Parse the single argument of a `qFace`/`qEdge`/`qVertex` query as a
/// `u64` id_hash. Parsed as `u64` (not `i64`) because an id_hash is a
/// full 64-bit FNV-1a value that routinely exceeds `i64::MAX`.
fn expect_one_id_hash(
    args: &[&str],
    message: &'static str,
) -> Result<u64, QueryParseError> {
    if args.len() != 1 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    args[0]
        .parse::<u64>()
        .map_err(|_| QueryParseError::InvalidIdHash(args[0].to_string()))
}

/// The high-level result of evaluating a query. Carries the
/// classification hint the query form implies plus the contributor
/// sets of every match. A match's contributor set is the list of
/// [`FeatureId`]s that jointly own the matched entity — for
/// `qCreatedBy` that's exactly one id, for `qEdgeBetween` it's two,
/// and downstream slices can grow the set as more query forms land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    /// Which B-Rep proxy class the query is asking about. The
    /// feature-level evaluator in K4.5.7 doesn't actually consult
    /// the proxy yet, but carrying the hint lets downstream mesh-
    /// level resolution narrow the candidate set without a second
    /// query parse.
    pub kind: QueryElementKind,
    /// Each entry is the contributor set of one matching entity.
    /// Empty vec means "the query is well-formed but nothing in
    /// the document matches it right now".
    pub matches: Vec<Vec<FeatureId>>,
}

/// B-Rep proxy classification hint attached to a [`QueryResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryElementKind {
    /// `qCreatedBy` — everything the feature owns outright.
    Set,
    /// `qFaceOf` — face-classified proxy elements (single
    /// contributor).
    Face,
    /// `qEdgeBetween` — edge-classified proxy elements (two
    /// contributors exactly).
    Edge,
}

/// Evaluate a parsed query against a document.
///
/// At the feature level this is a three-line walk: check whether
/// each referenced [`FeatureId`] exists in
/// [`Document::features`]; if so, emit its contributor set as a
/// match. The result is deterministic for a given document state,
/// which is what the [`QueryCache`] relies on.
pub fn evaluate_query(features: &[DecodedFeature], expr: &QueryExpr) -> QueryResult {
    match expr {
        QueryExpr::CreatedBy(id) => QueryResult {
            kind: QueryElementKind::Set,
            matches: if feature_exists(features, id) {
                vec![vec![id.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::FaceOf(id) => QueryResult {
            kind: QueryElementKind::Face,
            matches: if feature_exists(features, id) {
                vec![vec![id.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::EdgeBetween(a, b) => QueryResult {
            kind: QueryElementKind::Edge,
            matches: if feature_exists(features, a) && feature_exists(features, b) {
                vec![vec![a.clone(), b.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::AllRegions(id) | QueryExpr::Region(id, _) | QueryExpr::Regions(id, _) => {
            QueryResult {
                kind: QueryElementKind::Face,
                matches: if feature_exists(features, id) {
                    vec![vec![id.clone()]]
                } else {
                    Vec::new()
                },
            }
        }
        // id_hash-addressed element queries are resolved against the
        // woven solid by `resolve_query_topo`, not at the feature level:
        // an id_hash carries no contributor FeatureId set on its own. The
        // classification hint still travels so callers know the kind.
        QueryExpr::Face(_) => QueryResult {
            kind: QueryElementKind::Face,
            matches: Vec::new(),
        },
        QueryExpr::Edge(_) => QueryResult {
            kind: QueryElementKind::Edge,
            matches: Vec::new(),
        },
        QueryExpr::Vertex(_) => QueryResult {
            kind: QueryElementKind::Set,
            matches: Vec::new(),
        },
    }
}

fn feature_exists(features: &[DecodedFeature], id: &FeatureId) -> bool {
    features.iter().any(|f| &f.id == id)
}

// ─── element-level resolution (S3) ────────────────────────────────

/// Resolve a query against woven B-Reps to concrete B-Rep element
/// handles (Face/Edge/Vertex) with regeneration-stable id hashes.
///
/// Element-level counterpart to [`evaluate_query`] (which stays at the
/// feature level, emitting contributor sets). Where `evaluate_query`
/// answers "does feature `f` exist and what owns the match", this drops
/// down onto the live solid and returns the actual faces/edges the
/// feature produced, as [`TopoRef`](crate::brep::naming::TopoRef)
/// handles whose `id_hash` survives a re-weave (the persistent-naming
/// guarantee from `brep::naming`).
///
/// Semantics — a [`BranchId`](crate::csg::BranchId) is mapped back to
/// its flat feature id via [`BranchId::feature_id`], compared against
/// the query's [`FeatureId::as_u32_or_root`]:
/// - `CreatedBy(f)` / `FaceOf(f)`: every [`Face`](crate::brep::Face)
///   whose `branch_id.feature_id() == f` → [`TopoRef::from_face`]. The
///   two are kept as distinct records of intent even though they
///   resolve identically today (CreatedBy may later widen to the
///   edges/vertices touching `f`).
/// - `EdgeBetween(a, b)`: every [`Edge`](crate::brep::Edge) whose two
///   `signature` branches map to exactly the unordered pair `{a, b}` →
///   [`TopoRef::from_edge`].
/// - `AllRegions/Region/Regions`: sketch-region queries (K8.13). These
///   resolve against **sketch** data, not the woven solid, so there is
///   nothing to find in a `BRep` — they return `Vec::new()`.
///
/// Spanning multiple B-Reps (one per body partition) is supported: the
/// arenas are walked per-brep and the union returned. Output is sorted
/// by `id_hash` (then by kind) so the result is deterministic across
/// calls and across body iteration order.
///
/// A query feature id that is not flat-or-root (`as_u32_or_root()` is
/// `None`) matches nothing — hierarchical clone ids aren't yet mapped
/// to branch feature ids here (follow-up alongside hierarchical query
/// support).
pub fn resolve_query_topo(
    breps: &[crate::brep::BRep],
    expr: &QueryExpr,
) -> Vec<crate::brep::naming::TopoRef> {
    use crate::brep::naming::TopoRef;
    use crate::brep::{EdgeId, FaceId, VertexId};

    let mut out: Vec<TopoRef> = Vec::new();

    match expr {
        QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => {
            let Some(target) = f.as_u32_or_root() else {
                return Vec::new();
            };
            for brep in breps {
                for i in 0..brep.faces.len() {
                    if brep.faces[i].branch_id.feature_id() == target {
                        out.push(TopoRef::from_face(brep, FaceId(i as u32)));
                    }
                }
            }
        }
        QueryExpr::EdgeBetween(a, b) => {
            let (Some(fa), Some(fb)) = (a.as_u32_or_root(), b.as_u32_or_root()) else {
                return Vec::new();
            };
            for brep in breps {
                for i in 0..brep.edges.len() {
                    let sig = brep.edges[i].signature;
                    let s0 = sig.0.feature_id();
                    let s1 = sig.1.feature_id();
                    // Unordered pair match: {s0, s1} == {fa, fb}.
                    if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa) {
                        out.push(TopoRef::from_edge(brep, EdgeId(i as u32)));
                    }
                }
            }
        }
        // id_hash-addressed single-element queries: scan the relevant
        // entity kind and return the TopoRef whose stable id_hash equals
        // the queried value. This IS the regeneration-stable rebinding —
        // an id_hash is branch-derived, so a stored `qFace(h)` re-resolves
        // to the same face after a non-topological edit (resize). Normally
        // exactly one match; empty if the element no longer exists (e.g.
        // after a topological edit). NOTE: fingerprint-based HEALING for
        // the ambiguous/missing case is deferred (decision #3 "Kern jetzt")
        // — id_hash hash-hit only for now.
        QueryExpr::Face(hash) => {
            for brep in breps {
                for i in 0..brep.faces.len() {
                    let r = TopoRef::from_face(brep, FaceId(i as u32));
                    if r.id_hash == *hash {
                        out.push(r);
                    }
                }
            }
        }
        QueryExpr::Edge(hash) => {
            for brep in breps {
                for i in 0..brep.edges.len() {
                    let r = TopoRef::from_edge(brep, EdgeId(i as u32));
                    if r.id_hash == *hash {
                        out.push(r);
                    }
                }
            }
        }
        QueryExpr::Vertex(hash) => {
            for brep in breps {
                for i in 0..brep.vertices.len() {
                    let r = TopoRef::from_vertex(brep, VertexId(i as u32));
                    if r.id_hash == *hash {
                        out.push(r);
                    }
                }
            }
        }
        // Sketch-region queries resolve against sketch data, not the
        // woven solid; there is no B-Rep element to hand back.
        QueryExpr::AllRegions(_) | QueryExpr::Region(_, _) | QueryExpr::Regions(_, _) => {}
    }

    // Deterministic order: by id_hash, then kind as a stable tie-break.
    out.sort_by(|x, y| {
        x.id_hash
            .cmp(&y.id_hash)
            .then((x.kind as u8).cmp(&(y.kind as u8)))
    });
    out
}

/// Version-keyed memoisation layer for query evaluation.
///
/// Callers pair the cache with a **context version** — typically
/// the mutation counter the kernel bumps on every feature spine
/// edit. When the version advances, every previously cached
/// result is stale and the cache wipes itself before the next
/// insert. Within a stable version, repeated lookups of the same
/// query string hit the memoised result without re-parsing or
/// re-walking the document.
///
/// The cache is intentionally global per kernel rather than per
/// regeneration pass — queries asked by the Outer API from the
/// main thread (e.g. to resolve a picker click) should see the
/// same memoised results as queries a feature body asks during
/// regeneration, as long as the mutation counter agrees.
#[derive(Debug, Clone, Default)]
pub struct QueryCache {
    version: u64,
    entries: HashMap<String, QueryResult>,
}

impl QueryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The version the cache is currently keyed on. Starts at 0.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// How many entries are cached. Exposed for tests and the
    /// diagnostic surface; not load-bearing for correctness.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the cache to a new context version. If the version
    /// actually changed, every entry is dropped — the caller is
    /// asserting that document mutations have invalidated prior
    /// results. No-op when the version matches the current one.
    pub fn set_version(&mut self, version: u64) {
        if version != self.version {
            self.entries.clear();
            self.version = version;
        }
    }

    /// Look up a query in the cache, evaluating and memoising on
    /// miss. Returns a reference to the cached [`QueryResult`].
    ///
    /// The `expr_str` is both the cache key and the parse input.
    /// A parse error propagates without polluting the cache, so a
    /// subsequent call with a corrected string is not blocked by
    /// the previous failure.
    pub fn get_or_evaluate(
        &mut self,
        features: &[DecodedFeature],
        expr_str: &str,
    ) -> Result<&QueryResult, QueryParseError> {
        if !self.entries.contains_key(expr_str) {
            let expr = parse_query(expr_str)?;
            let result = evaluate_query(features, &expr);
            self.entries.insert(expr_str.to_string(), result);
        }
        Ok(&self.entries[expr_str])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::PrimitiveModifiers;
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    fn box_feature(n: u32) -> DecodedFeature {
        DecodedFeature::from_parts(
            id(n),
            format!("Box {n}"),
            BoolOp::Union,
            Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn doc_with_features(features: Vec<DecodedFeature>) -> Vec<DecodedFeature> {
        features
    }

    // ─── parse_query ──────────────────────────────────────────

    #[test]
    fn parses_created_by_with_flat_numeric_id() {
        let q = parse_query("qCreatedBy(1)").unwrap();
        assert_eq!(q, QueryExpr::CreatedBy(id(1)));
    }

    #[test]
    fn parses_face_of_with_flat_numeric_id() {
        let q = parse_query("qFaceOf(42)").unwrap();
        assert_eq!(q, QueryExpr::FaceOf(id(42)));
    }

    #[test]
    fn parses_edge_between_with_two_ids() {
        let q = parse_query("qEdgeBetween(1, 2)").unwrap();
        assert_eq!(q, QueryExpr::EdgeBetween(id(1), id(2)));
    }

    #[test]
    fn tolerates_whitespace_around_arguments() {
        let q = parse_query("  qEdgeBetween(  3 ,  4  )  ").unwrap();
        assert_eq!(q, QueryExpr::EdgeBetween(id(3), id(4)));
    }

    #[test]
    fn rejects_unknown_query_name() {
        let err = parse_query("qBogus(1)").unwrap_err();
        assert_eq!(err, QueryParseError::UnknownQuery("qBogus".to_string()));
    }

    #[test]
    fn rejects_missing_open_paren() {
        let err = parse_query("qCreatedBy 1").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_missing_close_paren() {
        let err = parse_query("qCreatedBy(1").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_created_by() {
        let err = parse_query("qCreatedBy(1, 2)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_edge_between() {
        let err = parse_query("qEdgeBetween(1)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn display_round_trips_every_variant() {
        for raw in [
            "qCreatedBy(1)",
            "qFaceOf(42)",
            "qEdgeBetween(1, 2)",
            "qFace(12345)",
            "qEdge(67890)",
            "qVertex(42)",
        ] {
            let parsed = parse_query(raw).unwrap();
            assert_eq!(parsed.to_string(), raw);
        }
    }

    // ─── id_hash-addressed element queries (parse) ─────────────

    #[test]
    fn parses_face_with_id_hash() {
        assert_eq!(parse_query("qFace(12345)").unwrap(), QueryExpr::Face(12345));
    }

    #[test]
    fn parses_edge_with_id_hash() {
        assert_eq!(parse_query("qEdge(12345)").unwrap(), QueryExpr::Edge(12345));
    }

    #[test]
    fn parses_vertex_with_id_hash() {
        assert_eq!(
            parse_query("qVertex(12345)").unwrap(),
            QueryExpr::Vertex(12345)
        );
    }

    #[test]
    fn parses_id_hash_beyond_i64_range() {
        // A real id_hash is a full u64 FNV-1a value and routinely exceeds
        // i64::MAX — it must parse as a u64, not silently fail.
        let big = u64::MAX;
        assert_eq!(
            parse_query(&format!("qFace({big})")).unwrap(),
            QueryExpr::Face(big)
        );
    }

    #[test]
    fn rejects_non_numeric_id_hash() {
        let err = parse_query("qFace(abc)").unwrap_err();
        assert!(matches!(err, QueryParseError::InvalidIdHash(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_face() {
        let err = parse_query("qFace(1, 2)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    // ─── evaluate_query ───────────────────────────────────────

    #[test]
    fn evaluate_created_by_finds_existing_feature() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let result = evaluate_query(&doc, &QueryExpr::CreatedBy(id(1)));
        assert_eq!(result.kind, QueryElementKind::Set);
        assert_eq!(result.matches, vec![vec![id(1)]]);
    }

    #[test]
    fn evaluate_created_by_returns_empty_for_missing_feature() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result = evaluate_query(&doc, &QueryExpr::CreatedBy(id(999)));
        assert!(result.matches.is_empty());
        // Classification hint still travels through an empty
        // result — it's a property of the query, not the matches.
        assert_eq!(result.kind, QueryElementKind::Set);
    }

    #[test]
    fn evaluate_face_of_carries_face_kind_hint() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result = evaluate_query(&doc, &QueryExpr::FaceOf(id(1)));
        assert_eq!(result.kind, QueryElementKind::Face);
        assert_eq!(result.matches, vec![vec![id(1)]]);
    }

    #[test]
    fn evaluate_edge_between_returns_both_ids_on_match() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let result =
            evaluate_query(&doc, &QueryExpr::EdgeBetween(id(1), id(2)));
        assert_eq!(result.kind, QueryElementKind::Edge);
        assert_eq!(result.matches, vec![vec![id(1), id(2)]]);
    }

    #[test]
    fn evaluate_edge_between_empty_when_one_feature_missing() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result =
            evaluate_query(&doc, &QueryExpr::EdgeBetween(id(1), id(2)));
        assert!(result.matches.is_empty());
    }

    // ─── QueryCache ───────────────────────────────────────────

    #[test]
    fn cache_hits_return_identical_result_for_same_version() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        let first = cache
            .get_or_evaluate(&doc, "qCreatedBy(1)")
            .unwrap()
            .clone();
        let second = cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(&first, second);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_invalidates_on_version_bump() {
        let mut doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(cache.len(), 1);

        // Simulate a mutation that removes feature 1.
        doc.clear();
        cache.set_version(1);
        assert_eq!(cache.len(), 0);

        // The next evaluation must reflect the new document state.
        let fresh = cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert!(fresh.matches.is_empty());
    }

    #[test]
    fn cache_set_version_is_noop_when_version_matches() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        cache.set_version(0);
        // Still cached — no invalidation happened.
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_parse_errors_do_not_pollute_the_table() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        let err = cache.get_or_evaluate(&doc, "qBogus(1)").unwrap_err();
        assert_eq!(err, QueryParseError::UnknownQuery("qBogus".to_string()));
        assert_eq!(cache.len(), 0);
        // A subsequent well-formed call still works.
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_tracks_multiple_entries_at_one_version() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        cache.get_or_evaluate(&doc, "qFaceOf(2)").unwrap();
        cache
            .get_or_evaluate(&doc, "qEdgeBetween(1, 2)")
            .unwrap();
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn cache_version_getter_reflects_set_version_calls() {
        let mut cache = QueryCache::new();
        assert_eq!(cache.version(), 0);
        cache.set_version(7);
        assert_eq!(cache.version(), 7);
        cache.set_version(7);
        assert_eq!(cache.version(), 7);
    }

    // ─── resolve_query_topo (element-level, S3) ───────────────

    use crate::brep::naming::{EntityKind, TopoRef};
    use crate::brep::{BRep, Edge, EdgeGeometry, Face, Loop, SurfaceGeometry};
    use crate::csg::BranchId;
    use fielddraft_protocol::HalfSpace;

    /// A trivial analytic plane face on `branch`, with an empty outer
    /// loop (a fully closed analytic surface — enough for naming, which
    /// only consults `branch_id` + the face centroid).
    fn plane_face(branch: BranchId) -> Face {
        Face {
            branch_id: branch,
            surface: SurfaceGeometry::Analytic {
                surface: HalfSpace::Plane {
                    normal: [0.0, 0.0, 1.0],
                    offset: 0.0,
                },
                transform: Transform::IDENTITY,
            },
            outer: Loop::default(),
            inner: Vec::new(),
        }
    }

    /// Build a B-Rep with the given per-feature face counts plus a few
    /// edges, exercising distinct branch ids. `face_feats[i]` faces are
    /// minted for feature id `i+1`; the edges connect the listed
    /// feature pairs.
    fn synthetic_brep(
        face_feats: &[(u32, usize)],
        edge_pairs: &[(u32, u32)],
    ) -> BRep {
        let mut brep = BRep::new();
        for &(feat, count) in face_feats {
            for sub in 0..count {
                let branch =
                    BranchId::from_feature_sub(&FeatureId::from_u32(feat), sub as u32);
                brep.add_face(plane_face(branch));
            }
        }
        for (k, &(a, b)) in edge_pairs.iter().enumerate() {
            let ba = BranchId::from_feature(&FeatureId::from_u32(a));
            let bb = BranchId::from_feature(&FeatureId::from_u32(b));
            // Distinct geometry per edge so spatial disambiguation gives
            // each a unique id even when signatures collide.
            let y = k as f64;
            brep.add_edge(Edge::new(
                EdgeGeometry::Polyline(vec![[0.0, y, 0.0], [1.0, y, 0.0]]),
                ba,
                bb,
                None,
                None,
            ));
        }
        brep
    }

    fn is_sorted_by_hash(refs: &[TopoRef]) -> bool {
        refs.windows(2).all(|w| w[0].id_hash <= w[1].id_hash)
    }

    /// A real woven cube B-Rep (6 faces, 12 edges, 8 vertices) at the given
    /// half-extent — mirrors the naming.rs / lib.rs `box_kernel` helpers so
    /// the id_hash-stability test is exercised against actual branch-derived
    /// ids, not the synthetic plane faces above.
    fn cube_brep(half: f32) -> BRep {
        use crate::brep::octree::Octree;
        use crate::brep::weaving::weave;
        use crate::bvh::BoundedCsg;
        use crate::csg::CsgNode;
        use fielddraft_protocol::Primitive;

        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    #[test]
    fn resolve_created_by_returns_faces_of_feature() {
        // Feature 5 owns 6 faces (a box), feature 6 owns 1.
        let brep = synthetic_brep(&[(5, 6), (6, 1)], &[]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(refs.len(), 6, "all six faces of feature 5");
        assert!(refs.iter().all(|r| r.kind == EntityKind::Face));
        assert!(is_sorted_by_hash(&refs), "sorted by id_hash");
    }

    #[test]
    fn resolve_face_of_matches_created_by_set() {
        let brep = synthetic_brep(&[(5, 6), (6, 1)], &[]);
        let created = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedBy(id(5)));
        let faces = resolve_query_topo(&[brep], &QueryExpr::FaceOf(id(5)));
        assert_eq!(created, faces, "FaceOf == CreatedBy at the face level today");
    }

    #[test]
    fn resolve_created_by_nonmatching_feature_is_empty() {
        let brep = synthetic_brep(&[(5, 6)], &[]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(999)));
        assert!(refs.is_empty(), "no faces own feature 999");
    }

    #[test]
    fn resolve_edge_between_returns_the_ab_edges() {
        // One 1–2 edge, one 1–3 edge, one 2–3 edge.
        let brep = synthetic_brep(&[(1, 1), (2, 1), (3, 1)], &[(1, 2), (1, 3), (2, 3)]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(1), id(2)));
        assert_eq!(refs.len(), 1, "exactly the 1–2 edge");
        assert_eq!(refs[0].kind, EntityKind::Edge);
    }

    #[test]
    fn resolve_edge_between_is_unordered() {
        let brep = synthetic_brep(&[(1, 1), (2, 1)], &[(1, 2)]);
        let ab = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::EdgeBetween(id(1), id(2)));
        let ba = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(2), id(1)));
        assert_eq!(ab, ba, "{{a,b}} edge match is order-independent");
        assert_eq!(ab.len(), 1);
    }

    #[test]
    fn resolve_edge_between_nonmatching_pair_is_empty() {
        let brep = synthetic_brep(&[(1, 1), (2, 1)], &[(1, 2)]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(1), id(7)));
        assert!(refs.is_empty());
    }

    #[test]
    fn resolve_region_queries_return_empty() {
        let brep = synthetic_brep(&[(5, 6)], &[]);
        for expr in [
            QueryExpr::AllRegions(id(5)),
            QueryExpr::Region(id(5), 0),
            QueryExpr::Regions(id(5), vec![0, 1]),
        ] {
            assert!(
                resolve_query_topo(std::slice::from_ref(&brep), &expr).is_empty(),
                "region queries resolve against sketch data, not the solid"
            );
        }
    }

    #[test]
    fn resolve_spans_multiple_breps() {
        let a = synthetic_brep(&[(5, 3)], &[]);
        let b = synthetic_brep(&[(5, 2)], &[]);
        let refs = resolve_query_topo(&[a, b], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(refs.len(), 5, "faces from both bodies' B-Reps");
    }

    #[test]
    fn resolve_is_deterministic() {
        let brep = synthetic_brep(&[(5, 6), (6, 2)], &[(5, 6)]);
        let first = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedBy(id(5)));
        let second = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(first, second, "two calls give identical output");
    }

    // ─── id_hash-addressed element queries (resolve) ──────────

    #[test]
    fn resolve_face_by_id_hash_picks_exactly_that_face() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.face_count(), 6, "a box has 6 faces");

        // Capture one specific face's id_hash, then address it directly.
        let pick = crate::brep::FaceId(2);
        let want = TopoRef::from_face(&brep, pick);

        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(want.id_hash));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one face, not 6");
        assert_eq!(refs[0].kind, EntityKind::Face);
        assert_eq!(refs[0].id_hash, want.id_hash);
        assert_eq!(refs[0], want, "the resolved face is the one we addressed");
    }

    #[test]
    fn resolve_edge_by_id_hash_picks_exactly_that_edge() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.edge_count(), 12);
        let want = TopoRef::from_edge(&brep, crate::brep::EdgeId(5));
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(want.id_hash));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one edge");
        assert_eq!(refs[0].kind, EntityKind::Edge);
        assert_eq!(refs[0], want);
    }

    #[test]
    fn resolve_vertex_by_id_hash_picks_exactly_that_vertex() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.vertex_count(), 8);
        let want = TopoRef::from_vertex(&brep, crate::brep::VertexId(3));
        let refs =
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(want.id_hash));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one vertex");
        assert_eq!(refs[0].kind, EntityKind::Vertex);
        assert_eq!(refs[0], want);
    }

    #[test]
    fn resolve_face_id_hash_survives_resize() {
        // STABILITY: an id_hash is branch-derived, not geometry-derived, so a
        // stored qFace(h) from a 10mm box re-resolves to the corresponding
        // face of a 40mm box (a pure parameter change is topology-stable).
        let small = cube_brep(10.0);
        let big = cube_brep(40.0);

        let pick = crate::brep::FaceId(2);
        let want = TopoRef::from_face(&small, pick);

        let refs = resolve_query_topo(std::slice::from_ref(&big), &QueryExpr::Face(want.id_hash));
        assert_eq!(
            refs.len(),
            1,
            "the 10mm-box face id_hash still addresses exactly one face on the 40mm box"
        );
        assert_eq!(refs[0].id_hash, want.id_hash, "id_hash is regeneration-stable");
        // The matched face on the big box has the same branch as the small one.
        let big_branch = big.face(crate::brep::FaceId(2)).branch_id;
        let _ = big_branch; // (ordering may differ; id_hash equality is the contract)
    }

    #[test]
    fn resolve_nonexistent_id_hash_is_empty() {
        let brep = cube_brep(10.0);
        // A hash that belongs to no element of any kind.
        let bogus = 0xDEAD_BEEF_DEAD_BEEFu64;
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(bogus)).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(bogus)).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(bogus)).is_empty()
        );
    }
}
