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
//! * [`resolve_query_topo`] / [`locate_element`] — the element-level
//!   resolver against the woven B-Reps (fingerprint healing included).
//!   A feature-level evaluator (`evaluate_query`) and a version-keyed
//!   `QueryCache` shipped with K4.5.7 but never gained a consumer and
//!   were removed (YAGNI, 2026-07-07) — the weave cache already keys
//!   the expensive work, and every real consumer needs geometry.
//!
//! ## Deliberately not shipped
//!
//! * Hierarchical queries (`qCreatedBy(extrude_3.op.mask_0)`).
//!   [`FeatureId`]'s hierarchical form is preserved through
//!   `FromStr`, so the parser already accepts the path syntax —
//!   the evaluator just treats every id as an opaque lookup key.

use std::fmt;

use fielddraft_protocol::FeatureId;


use crate::brep::naming::Fingerprint;

/// A persisted element reference: the stable `id_hash` plus the geometric
/// [`Fingerprint`] captured at pick time (plan `PERSISTENT-NAMING-PLAN.md`
/// N2, finally cashing in DL-35). The fingerprint is what lets
/// [`resolve_query_topo`] *heal* the reference after a topological edit —
/// without it, resolution is exact-hash-only (the legacy short form
/// `qFace(<hash>)`, still accepted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementRef {
    pub id_hash: u64,
    pub fingerprint: Option<Fingerprint>,
}

impl ElementRef {
    pub fn hash_only(id_hash: u64) -> Self {
        Self { id_hash, fingerprint: None }
    }
}

impl From<u64> for ElementRef {
    fn from(id_hash: u64) -> Self {
        Self::hash_only(id_hash)
    }
}

impl From<&crate::brep::naming::TopoRef> for ElementRef {
    fn from(r: &crate::brep::naming::TopoRef) -> Self {
        Self { id_hash: r.id_hash, fingerprint: Some(r.fingerprint) }
    }
}

/// Parsed form of a `q…(…)` query string. Three variants match the
/// README's "query minimum" vocabulary.
///
/// No `Eq`: element references carry `f64` fingerprint data.
#[derive(Debug, Clone, PartialEq)]
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
    /// `qFace(id_hash [@ fp])` — the single B-Rep face whose stable
    /// [`TopoRef::id_hash`](crate::brep::naming::TopoRef) equals the
    /// given value. This is the id_hash-addressed form the UI emits
    /// when a user clicks exactly ONE face, and that a feature stores
    /// to reference it across regenerations. The optional `@`-suffixed
    /// fingerprint block (`cx cy cz size`; edges append `tx ty tz`)
    /// enables healing when the hash no longer resolves exactly.
    Face(ElementRef),
    /// `qEdge(id_hash [@ fp])` — the single B-Rep edge addressed by its
    /// stable id_hash, optionally fingerprint-carrying.
    Edge(ElementRef),
    /// `qVertex(id_hash [@ fp])` — the single B-Rep vertex addressed by
    /// its stable id_hash, optionally fingerprint-carrying.
    Vertex(ElementRef),
    /// `qSketchLine(sketch_id, line_id)` — one straight `sketch_line`
    /// sub-call of a committed sketch, addressed by the sketch feature
    /// id and the line's stable entity id. Unlike `qEdge`, this targets
    /// raw sketch geometry (including construction lines, which never
    /// reach the B-Rep) — the form the Revolve tool stores when the user
    /// picks a sketch line as the revolve axis.
    SketchLine(FeatureId, u32),
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
            QueryExpr::Face(r) => write_element_ref(f, "qFace", r),
            QueryExpr::Edge(r) => write_element_ref(f, "qEdge", r),
            QueryExpr::Vertex(r) => write_element_ref(f, "qVertex", r),
            QueryExpr::SketchLine(sketch, line) => write!(f, "qSketchLine({sketch}, {line})"),
        }
    }
}

/// Canonical string form of an element reference: `qFace(<hash>)` without a
/// fingerprint, `qFace(<hash> @ cx cy cz size [tx ty tz])` with one. Rust's
/// `f64` Display is shortest-round-trip, so the floats survive the string
/// round trip bit-exactly. The tangent triple is only emitted when non-zero
/// (edges); faces/vertices stay at four numbers.
fn write_element_ref(f: &mut fmt::Formatter<'_>, name: &str, r: &ElementRef) -> fmt::Result {
    match &r.fingerprint {
        None => write!(f, "{name}({})", r.id_hash),
        Some(fp) => {
            write!(
                f,
                "{name}({} @ {} {} {} {}",
                r.id_hash, fp.centroid[0], fp.centroid[1], fp.centroid[2], fp.size
            )?;
            if fp.tangent.iter().any(|&t| t != 0.0) {
                write!(f, " {} {} {}", fp.tangent[0], fp.tangent[1], fp.tangent[2])?;
            }
            write!(f, ")")
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
    /// The `@`-suffixed fingerprint block of an element query was not a
    /// list of 4 (face/vertex) or 7 (edge) floats. Carries the block.
    InvalidFingerprint(String),
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
            QueryParseError::InvalidFingerprint(raw) => {
                write!(f, "invalid fingerprint block in query: `{raw}`")
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
        "qFace" => Ok(QueryExpr::Face(expect_one_element_ref(
            &args,
            "qFace expects one id_hash",
        )?)),
        "qEdge" => Ok(QueryExpr::Edge(expect_one_element_ref(
            &args,
            "qEdge expects one id_hash",
        )?)),
        "qVertex" => Ok(QueryExpr::Vertex(expect_one_element_ref(
            &args,
            "qVertex expects one id_hash",
        )?)),
        "qSketchLine" => {
            let (a, b) = expect_two_ids(&args, "qSketchLine expects sketch_id and line_id")?;
            let line = b.as_u32_if_flat().ok_or(QueryParseError::InvalidFeatureId(
                "line id must be a plain number".to_string(),
            ))?;
            Ok(QueryExpr::SketchLine(a, line))
        }
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

/// Parse the single argument of a `qFace`/`qEdge`/`qVertex` query as an
/// [`ElementRef`]: a `u64` id_hash, optionally followed by an `@`-separated
/// fingerprint block of 4 (face/vertex) or 7 (edge) whitespace-separated
/// floats. The hash is parsed as `u64` (not `i64`) because an id_hash is a
/// full 64-bit FNV-1a value that routinely exceeds `i64::MAX`.
fn expect_one_element_ref(
    args: &[&str],
    message: &'static str,
) -> Result<ElementRef, QueryParseError> {
    if args.len() != 1 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    let (hash_raw, fp_raw) = match args[0].split_once('@') {
        Some((h, fp)) => (h.trim(), Some(fp.trim())),
        None => (args[0], None),
    };
    let id_hash = hash_raw
        .parse::<u64>()
        .map_err(|_| QueryParseError::InvalidIdHash(hash_raw.to_string()))?;
    let fingerprint = match fp_raw {
        None => None,
        Some(block) => {
            let nums: Result<Vec<f64>, _> =
                block.split_whitespace().map(|t| t.parse::<f64>()).collect();
            let nums =
                nums.map_err(|_| QueryParseError::InvalidFingerprint(block.to_string()))?;
            let tangent = match nums.len() {
                4 => [0.0; 3],
                7 => [nums[4], nums[5], nums[6]],
                _ => return Err(QueryParseError::InvalidFingerprint(block.to_string())),
            };
            Some(Fingerprint {
                centroid: [nums[0], nums[1], nums[2]],
                size: nums[3],
                tangent,
            })
        }
    };
    Ok(ElementRef { id_hash, fingerprint })
}

/// The element-level query resolver. Where the (removed, YAGNI) feature-
/// level evaluator only echoed contributor ids, this
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
    use crate::brep::{EdgeId, FaceId};

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
        // id_hash-addressed single-element queries. Two regimes
        // (plan PERSISTENT-NAMING-PLAN.md N3):
        //
        // - **Fingerprint-carrying reference** (the canonical long form):
        //   run the full `naming::resolve` state machine per body and bind
        //   to the best `Resolved`/`Healed` outcome. This both survives
        //   topological edits (healing) and refuses the swapped-hash-carrier
        //   misbinding (the resolver's geometric override). `Ambiguous` and
        //   `Missing` yield nothing — the caller reports the reference as
        //   unresolved instead of acting on an arbitrary pick.
        // - **Hash-only reference** (legacy short form, or programmatic
        //   `fd.q.face(hash)`): exact hash scan, as before — there is no
        //   stored geometry to heal against.
        QueryExpr::Face(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Face, eref, &mut out)
        }
        QueryExpr::Edge(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Edge, eref, &mut out)
        }
        QueryExpr::Vertex(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Vertex, eref, &mut out)
        }
        // Sketch-region and sketch-line queries resolve against sketch
        // data, not the woven solid; there is no B-Rep element to hand back.
        QueryExpr::AllRegions(_)
        | QueryExpr::Region(_, _)
        | QueryExpr::Regions(_, _)
        | QueryExpr::SketchLine(_, _) => {}
    }

    // Deterministic order: by id_hash, then kind as a stable tie-break.
    out.sort_by(|x, y| {
        x.id_hash
            .cmp(&y.id_hash)
            .then((x.kind as u8).cmp(&(y.kind as u8)))
    });
    out
}

/// Resolve one id_hash-addressed element reference against all bodies —
/// the two-regime logic described at the `Face`/`Edge`/`Vertex` arms of
/// [`resolve_query_topo`].
fn resolve_element(
    breps: &[crate::brep::BRep],
    kind: crate::brep::naming::EntityKind,
    eref: &ElementRef,
    out: &mut Vec<crate::brep::naming::TopoRef>,
) {
    use crate::brep::naming::{EntityHandle, TopoRef};
    if let Some((bi, handle)) = locate_element(breps.iter(), kind, eref) {
        let brep = &breps[bi];
        out.push(match handle {
            EntityHandle::Face(id) => TopoRef::from_face(brep, id),
            EntityHandle::Edge(id) => TopoRef::from_edge(brep, id),
            EntityHandle::Vertex(id) => TopoRef::from_vertex(brep, id),
        });
    }
}

/// The canonical fingerprint-carrying element query for a [`TopoRef`] —
/// what a viewport pick should persist so the reference can heal later.
/// The single authority for the string form is [`QueryExpr`]'s `Display`.
pub fn element_query(r: &crate::brep::naming::TopoRef) -> QueryExpr {
    use crate::brep::naming::EntityKind;
    let eref = ElementRef::from(r);
    match r.kind {
        EntityKind::Face => QueryExpr::Face(eref),
        EntityKind::Edge => QueryExpr::Edge(eref),
        EntityKind::Vertex => QueryExpr::Vertex(eref),
    }
}

/// Locate the single element an [`ElementRef`] names across `breps`,
/// returning `(body index, arena handle)`. The shared primitive behind
/// [`resolve_query_topo`] and every `Context::resolve_*` face/edge/vertex
/// path — one implementation of the fingerprint-healing regime instead of
/// a hash-scan copy per feature.
///
/// - With a fingerprint: run [`naming::resolve`](resolve) per body and take
///   the best `Resolved`/`Healed` outcome (`Ambiguous`/`Missing` → `None`).
/// - Hash-only: first exact `id_hash` match wins, as before.
pub fn locate_element<'a, I>(
    breps: I,
    kind: crate::brep::naming::EntityKind,
    eref: &ElementRef,
) -> Option<(usize, crate::brep::naming::EntityHandle)>
where
    I: IntoIterator<Item = &'a crate::brep::BRep>,
{
    use crate::brep::naming::{resolve, EntityHandle, EntityKind, ResolutionState, TopoRef};
    use crate::brep::{EdgeId, FaceId, VertexId};

    match eref.fingerprint {
        Some(fingerprint) => {
            let stored = TopoRef { kind, id_hash: eref.id_hash, fingerprint };
            let mut best: Option<(f64, usize, EntityHandle)> = None;
            for (bi, brep) in breps.into_iter().enumerate() {
                let res = resolve(&stored, brep);
                if !matches!(res.state, ResolutionState::Resolved | ResolutionState::Healed) {
                    continue;
                }
                let Some(handle) = res.handle else { continue };
                if best.as_ref().is_none_or(|(s, _, _)| res.score > *s) {
                    best = Some((res.score, bi, handle));
                }
            }
            best.map(|(_, bi, h)| (bi, h))
        }
        None => {
            for (bi, brep) in breps.into_iter().enumerate() {
                let n = match kind {
                    EntityKind::Face => brep.faces.len(),
                    EntityKind::Edge => brep.edges.len(),
                    EntityKind::Vertex => brep.vertices.len(),
                };
                for i in 0..n {
                    let (r, handle) = match kind {
                        EntityKind::Face => {
                            let id = FaceId(i as u32);
                            (TopoRef::from_face(brep, id), EntityHandle::Face(id))
                        }
                        EntityKind::Edge => {
                            let id = EdgeId(i as u32);
                            (TopoRef::from_edge(brep, id), EntityHandle::Edge(id))
                        }
                        EntityKind::Vertex => {
                            let id = VertexId(i as u32);
                            (TopoRef::from_vertex(brep, id), EntityHandle::Vertex(id))
                        }
                    };
                    if r.id_hash == eref.id_hash {
                        return Some((bi, handle));
                    }
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{FeatureId, Transform};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
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
    fn parses_and_roundtrips_sketch_line() {
        let q = parse_query("qSketchLine(7, 42)").unwrap();
        assert_eq!(q, QueryExpr::SketchLine(id(7), 42));
        // Display is the canonical wire form and must re-parse.
        assert_eq!(q.to_string(), "qSketchLine(7, 42)");
        assert_eq!(parse_query(&q.to_string()).unwrap(), q);
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
        assert_eq!(parse_query("qFace(12345)").unwrap(), QueryExpr::Face(12345.into()));
    }

    #[test]
    fn parses_edge_with_id_hash() {
        assert_eq!(parse_query("qEdge(12345)").unwrap(), QueryExpr::Edge(12345.into()));
    }

    #[test]
    fn parses_vertex_with_id_hash() {
        assert_eq!(
            parse_query("qVertex(12345)").unwrap(),
            QueryExpr::Vertex(12345.into())
        );
    }

    #[test]
    fn parses_id_hash_beyond_i64_range() {
        // A real id_hash is a full u64 FNV-1a value and routinely exceeds
        // i64::MAX — it must parse as a u64, not silently fail.
        let big = u64::MAX;
        assert_eq!(
            parse_query(&format!("qFace({big})")).unwrap(),
            QueryExpr::Face(big.into())
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

        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(want.id_hash.into()));
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
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(want.id_hash.into()));
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
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(want.id_hash.into()));
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

        let refs = resolve_query_topo(std::slice::from_ref(&big), &QueryExpr::Face(want.id_hash.into()));
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

    // ─── fingerprint-carrying element refs (N2/N3) ──────────

    #[test]
    fn element_query_long_form_round_trips() {
        let brep = cube_brep(10.0);
        for q in [
            element_query(&TopoRef::from_face(&brep, crate::brep::FaceId(2))),
            element_query(&TopoRef::from_edge(&brep, crate::brep::EdgeId(5))),
            element_query(&TopoRef::from_vertex(&brep, crate::brep::VertexId(3))),
        ] {
            let s = q.to_string();
            let parsed = parse_query(&s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"));
            assert_eq!(parsed, q, "Display ↔ parse_query must round-trip `{s}`");
        }
    }

    #[test]
    fn short_form_stays_hash_only() {
        let q = parse_query("qFace(12345)").unwrap();
        assert_eq!(q, QueryExpr::Face(ElementRef::hash_only(12345)));
        assert_eq!(q.to_string(), "qFace(12345)");
    }

    #[test]
    fn malformed_fingerprint_block_is_rejected() {
        for bad in [
            "qFace(12345 @ 1 2 3)",       // wrong arity (3)
            "qFace(12345 @ 1 2 3 4 5)",   // wrong arity (5)
            "qEdge(12345 @ a b c d)",     // non-numeric
        ] {
            assert!(
                matches!(parse_query(bad), Err(QueryParseError::InvalidFingerprint(_))),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn fingerprint_ref_heals_across_hash_loss() {
        // The stored hash belongs to no element of the rebuilt B-Rep (a
        // bogus hash stands in for a topology change that renamed the
        // element), but the fingerprint pins the geometry — the reference
        // heals to the fingerprint-nearest element instead of vanishing.
        let brep = cube_brep(10.0);
        let want = TopoRef::from_face(&brep, crate::brep::FaceId(2));
        let eref = ElementRef {
            id_hash: 0xDEAD_BEEF_DEAD_BEEFu64,
            fingerprint: Some(want.fingerprint),
        };
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(eref));
        assert_eq!(refs.len(), 1, "the fingerprint must heal the lost hash");
        assert_eq!(refs[0], want, "healing binds to the fingerprint's face");
    }

    #[test]
    fn fingerprint_ref_without_match_is_empty() {
        // Bogus hash AND a fingerprint far outside the body: nothing to
        // heal towards — the reference reports unresolved (no arbitrary pick).
        let brep = cube_brep(10.0);
        let eref = ElementRef {
            id_hash: 0xDEAD_BEEF_DEAD_BEEFu64,
            fingerprint: Some(crate::brep::naming::Fingerprint {
                centroid: [1e6, 1e6, 1e6],
                size: 1e-3,
                tangent: [0.0; 3],
            }),
        };
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(eref)).is_empty(),
            "a reference that matches nothing must not bind"
        );
    }

    #[test]
    fn resolve_nonexistent_id_hash_is_empty() {
        let brep = cube_brep(10.0);
        // A hash that belongs to no element of any kind.
        let bogus = 0xDEAD_BEEF_DEAD_BEEFu64;
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(bogus.into())).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(bogus.into())).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(bogus.into())).is_empty()
        );
    }
}
