//! FieldDraft wire protocol — shared types for kernel ↔ frontend.
//!
//! This crate stays `no_std`-friendly-ish and avoids any wasm-bindgen deps,
//! so the same types can be consumed from the kernel crate and (later) a
//! JSON-schema export for the TypeScript side.

use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub mod mesh_sdf;
pub mod sketch2d;
pub use mesh_sdf::MeshSdf;
pub use sketch2d::{Aabb2, Sketch2d, Sketch2dPrimitive};

/// Current on-disk / on-wire schema version for the project document.
pub const SCHEMA_VERSION: u32 = 2;

/// Hierarchical, typed identifier for a feature or one of its sub-nodes.
///
/// The K4.5 "Feature-as-Function" refactor replaces the flat `u32` ids
/// from K1/K2/K4 with a tree-structured id so sub-operations inside a
/// feature body can carve out their own stable namespace. Concretely:
///
/// * `parent` is an optional ancestor id — `None` for top-level
///   feature entries in the document, `Some` for sub-nodes created
///   inside a feature body.
/// * `local` is the short per-level name stored as [`SmolStr`] so
///   the common flat-numeric form (`"1"`, `"42"`) stays inline-
///   allocated without hitting the heap.
///
/// Pretty-printed as a dotted path (`extrude_3.op`, `fillet_7.mask_0`)
/// via the [`fmt::Display`] impl. Parseable back via [`FromStr`] with
/// the mirror rules.
///
/// ## Backward compatibility with K1 documents
///
/// Old `fielddraft` JSON documents serialise feature ids as bare
/// integers (`"id": 3`). The custom [`Serialize`] / [`Deserialize`]
/// impls below keep that shape for flat-numeric ids (those with no
/// parent whose `local` parses as `u32`), and reject nothing on the
/// deserialiser side — bare numbers, dotted strings, and bare strings
/// are all accepted. The K4.5.1 slice never creates hierarchical
/// ids, so every document in the wild still round-trips to the
/// exact same bytes it had under K1.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct FeatureId {
    pub parent: Option<Box<FeatureId>>,
    pub local: SmolStr,
}

impl FeatureId {
    /// Build a top-level id from a textual local name.
    pub fn root(local: impl Into<SmolStr>) -> Self {
        Self {
            parent: None,
            local: local.into(),
        }
    }

    /// Build a top-level id from a flat numeric counter — the
    /// classic K1-style id, just typed. Used by
    /// [`Document::allocate_id`] to keep the existing
    /// monotonic-counter allocation scheme alive inside the new
    /// type.
    pub fn from_u32(n: u32) -> Self {
        Self::root(n.to_string())
    }

    /// Build a child id underneath this one. The new id keeps the
    /// whole ancestor chain so `Display` can emit the full path.
    pub fn child(&self, local: impl Into<SmolStr>) -> Self {
        Self {
            parent: Some(Box::new(self.clone())),
            local: local.into(),
        }
    }

    /// Return the parent id, if any.
    pub fn parent(&self) -> Option<&FeatureId> {
        self.parent.as_deref()
    }

    /// `true` when this id has no parent and the local part parses
    /// cleanly as a `u32`. This is the "compat-shim" predicate:
    /// [`Self::as_u32_if_flat`] uses it to decide whether the id
    /// can still go through the K1-era `u32` wasm-bindgen surface.
    pub fn is_flat_numeric(&self) -> bool {
        self.parent.is_none() && self.local.parse::<u32>().is_ok()
    }

    /// Collapse the id back to a `u32`, assuming it's flat-numeric.
    /// Returns `None` for hierarchical or non-numeric ids. The
    /// wasm-bindgen surface in `crates/kernel/src/lib.rs` uses this
    /// to keep the existing `u32` signatures working during K4.5.1
    /// — every id issued in this slice is still flat-numeric, so
    /// the `.expect` paths are unreachable for well-behaved inputs.
    pub fn as_u32_if_flat(&self) -> Option<u32> {
        if self.parent.is_some() {
            return None;
        }
        self.local.parse::<u32>().ok()
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(parent) = &self.parent {
            write!(f, "{parent}.{}", self.local)
        } else {
            f.write_str(&self.local)
        }
    }
}

/// Parse error for [`FeatureId`]. A future hierarchical slice may
/// grow this with richer variants (empty segments, forbidden chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureIdParseError {
    pub message: &'static str,
}

impl fmt::Display for FeatureIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for FeatureIdParseError {}

impl From<ParseIntError> for FeatureIdParseError {
    fn from(_: ParseIntError) -> Self {
        Self {
            message: "invalid numeric feature id",
        }
    }
}

impl FromStr for FeatureId {
    type Err = FeatureIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(FeatureIdParseError {
                message: "empty feature id",
            });
        }
        // Split at the *last* dot so parent chains parse left-
        // to-right but build right-to-left, matching the
        // [`Display`] shape `"grandparent.parent.local"`.
        match s.rsplit_once('.') {
            None => Ok(FeatureId::root(s)),
            Some((head, tail)) => {
                if tail.is_empty() {
                    return Err(FeatureIdParseError {
                        message: "empty local segment in feature id",
                    });
                }
                let parent = FeatureId::from_str(head)?;
                Ok(FeatureId {
                    parent: Some(Box::new(parent)),
                    local: SmolStr::from(tail),
                })
            }
        }
    }
}

impl Serialize for FeatureId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Keep byte-identical shape with K1 documents: a flat
        // numeric id serialises as a bare integer, anything else
        // as the dotted-path string form.
        if let Some(n) = self.as_u32_if_flat() {
            serializer.serialize_u32(n)
        } else {
            serializer.collect_str(self)
        }
    }
}

impl<'de> Deserialize<'de> for FeatureId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FeatureIdVisitor;

        impl<'de> Visitor<'de> for FeatureIdVisitor {
            type Value = FeatureId;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a feature id: integer or dotted string")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<FeatureId, E> {
                Ok(FeatureId::from_u32(v as u32))
            }

            fn visit_u32<E: de::Error>(self, v: u32) -> Result<FeatureId, E> {
                Ok(FeatureId::from_u32(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<FeatureId, E> {
                if v < 0 {
                    return Err(E::custom("negative feature id"));
                }
                Ok(FeatureId::from_u32(v as u32))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<FeatureId, E> {
                FeatureId::from_str(v).map_err(E::custom)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<FeatureId, E> {
                FeatureId::from_str(&v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(FeatureIdVisitor)
    }
}

/// A single SDF primitive shape. The primitive itself carries no
/// boolean operation — that lives on its enclosing `Feature`.
///
/// K7.3 adds the `Extrude` variant so the extrude feature can
/// lower into a real sketch-profile volume rather than a
/// rectangular Box stand-in. The sketch payload is a full
/// [`Sketch2d`] tree (including unions / differences), and
/// `half_depth_z` plus `center` define the world-space slab
/// the profile sweeps along the local Z axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    Box { center: [f32; 3], half_extents: [f32; 3] },
    Sphere { center: [f32; 3], radius: f32 },
    Cylinder { center: [f32; 3], radius: f32, half_height: f32 },
    /// K7.3: sketch-profile extrusion. The `sketch` field carries
    /// the 2D region that gets swept; `half_depth_z` is half the
    /// total extrude height along the local Z axis; `center`
    /// positions the resulting slab in world space (the extrude
    /// plane is at `center[2]`).
    Extrude {
        sketch: Sketch2d,
        half_depth_z: f32,
        center: [f32; 3],
    },
    /// Torus centred at the origin, major radius R (centre of
    /// tube to axis), minor radius r (tube cross-section).
    /// SDF: `length(vec2(length(p.xz) - R, p.y)) - r`.
    Torus {
        center: [f32; 3],
        major_radius: f32,
        minor_radius: f32,
    },
    /// Regular hexagonal prism along the Y axis. `apothem` is the
    /// center-to-flat distance (= circumradius · cos(30°)). Used
    /// for hex bolts, nuts, Allen keys.
    HexPrism {
        apothem: f32,
        half_height: f32,
    },
    /// Capsule (line-segment swept sphere) along the Y axis.
    /// `half_height` is the half-length of the inner segment
    /// (total height = 2 * half_height + 2 * radius).
    Capsule {
        radius: f32,
        half_height: f32,
    },
    /// Truncated cone (frustum) with bottom radius `r_bottom`,
    /// top radius `r_top`, and half-height along Y. When
    /// `r_top == 0` it's a perfect cone.
    Cone {
        r_bottom: f32,
        r_top: f32,
        half_height: f32,
    },
    /// K12: metric thread via direct helix formula. O(1) per
    /// sample — no sweep, no path discretisation. The groove
    /// is carved into a cylinder of `nominal_radius` with a
    /// helical pitch and thread depth.
    Thread {
        nominal_radius: f32,
        pitch: f32,
        depth: f32,
        half_height: f32,
    },
    /// K13.1: imported triangle mesh as a CSG-participating solid.
    /// The `MeshSdf` carries the flat triangle buffer + per-tri
    /// AABBs and provides an O(N) nearest-triangle + ray-parity
    /// SDF. Serialised inline — no external file reference.
    ImportedMesh {
        mesh: Box<MeshSdf>,
    },
    /// K11.0: Gyroid TPMS lattice. The raw implicit is
    /// `sin(x/s)cos(y/s) + sin(y/s)cos(z/s) + sin(z/s)cos(x/s)`,
    /// thickened into a solid shell via `|f| - thickness`.
    /// The field repeats infinitely — intersect with a host body
    /// to clip it to a finite region.
    Gyroid {
        cell_size: f32,
        thickness: f32,
    },
    /// K8.4: sketch-profile revolution around the local Y axis.
    /// The sketch `x` coordinate maps to the radial distance
    /// `√(x² + z²)` in 3D, its `y` stays vertical. Authoring
    /// convention: sketch primitives should live at `x ≥ 0`;
    /// primitives that cross the axis produce valid but
    /// geometrically silly results. `center` positions the axis
    /// of revolution in world space.
    Revolve {
        sketch: Sketch2d,
        center: [f32; 3],
    },
}

impl Primitive {
    /// Numeric kind tag matching the packed-buffer / shader contract.
    pub const KIND_BOX: u32 = 0;
    pub const KIND_SPHERE: u32 = 1;
    pub const KIND_CYLINDER: u32 = 2;
    pub const KIND_EXTRUDE: u32 = 3;
    pub const KIND_REVOLVE: u32 = 4;
    pub const KIND_GYROID: u32 = 5;
    pub const KIND_TORUS: u32 = 6;
    pub const KIND_THREAD: u32 = 7;
    pub const KIND_CONE: u32 = 8;
    pub const KIND_CAPSULE: u32 = 9;
    pub const KIND_HEX_PRISM: u32 = 10;
    pub const KIND_IMPORTED_MESH: u32 = 11;

    pub fn kind_tag(&self) -> u32 {
        match self {
            Primitive::Box { .. } => Self::KIND_BOX,
            Primitive::Sphere { .. } => Self::KIND_SPHERE,
            Primitive::Cylinder { .. } => Self::KIND_CYLINDER,
            Primitive::Extrude { .. } => Self::KIND_EXTRUDE,
            Primitive::HexPrism { .. } => Self::KIND_HEX_PRISM,
            Primitive::Capsule { .. } => Self::KIND_CAPSULE,
            Primitive::Torus { .. } => Self::KIND_TORUS,
            Primitive::Cone { .. } => Self::KIND_CONE,
            Primitive::Thread { .. } => Self::KIND_THREAD,
            Primitive::Gyroid { .. } => Self::KIND_GYROID,
            Primitive::Revolve { .. } => Self::KIND_REVOLVE,
            Primitive::ImportedMesh { .. } => Self::KIND_IMPORTED_MESH,
        }
    }

    pub fn default_name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "Box",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Extrude { .. } => "Extrude",
            Primitive::HexPrism { .. } => "Hex prism",
            Primitive::Capsule { .. } => "Capsule",
            Primitive::Torus { .. } => "Torus",
            Primitive::Cone { .. } => "Cone",
            Primitive::Thread { .. } => "Thread",
            Primitive::Gyroid { .. } => "Gyroid",
            Primitive::Revolve { .. } => "Revolve",
            Primitive::ImportedMesh { .. } => "Imported mesh",
        }
    }
}

/// Boolean operation that combines a feature with the running result.
///
/// Evaluation order is the feature list itself: each feature folds into
/// the accumulator using its op. The very first `Union` feature seeds the
/// accumulator from `+∞` (empty space), so a list of `[Union(box)]`
/// renders that single box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    Union,
    Difference,
    Intersect,
}

impl BoolOp {
    /// Numeric tag for the packed buffer / shader contract.
    pub const TAG_UNION: u32 = 0;
    pub const TAG_DIFFERENCE: u32 = 1;
    pub const TAG_INTERSECT: u32 = 2;

    pub fn tag(self) -> u32 {
        match self {
            BoolOp::Union => Self::TAG_UNION,
            BoolOp::Difference => Self::TAG_DIFFERENCE,
            BoolOp::Intersect => Self::TAG_INTERSECT,
        }
    }
}

// ─── K4.5.4f: typed runtime parameter types ──────────────────────
//
// Length, Angle, and Query are the first typed parameter values in
// the FieldDraft feature system. They grow the FeatureSchema derive
// beyond the plain-scalar vocabulary of K4.5.4a (f32, [f32;3], bool,
// String, BoolOp) so that upcoming Rust-native feature definitions
// (Extrude, Revolve, hole patterns, …) can describe their parameters
// in terms of physical quantities and geometric references rather
// than bare numbers and strings.
//
// The model unit for lengths is millimetres (the kernel's native
// distance scale); `Length::to_mm` resolves any unit to this scale
// so feature execute paths can always work in model space. The
// model unit for angles is radians. Conversions are lossy (f32),
// matching the rest of the kernel.
//
// `Query` is a placeholder for K4.5.7's query minimum
// (`qCreatedBy`/`qFaceOf`/`qEdgeBetween`). Step 4f only introduces
// the wire type so features can declare query-shaped parameters;
// the actual evaluation against the contributor graph lands with
// the runtime K4.5.7 work.

/// Length unit, wire-encoded as a lowercase string (`"mm"`, `"cm"`,
/// `"m"`, `"in"`). The model unit is millimetres — [`Length::to_mm`]
/// converts any unit to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    Mm,
    Cm,
    M,
    In,
}

/// A length value with an explicit unit. Wire form is
/// `{"value": 10.0, "unit": "mm"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    pub const fn mm(value: f32) -> Self {
        Self { value, unit: LengthUnit::Mm }
    }
    pub const fn cm(value: f32) -> Self {
        Self { value, unit: LengthUnit::Cm }
    }
    pub const fn m(value: f32) -> Self {
        Self { value, unit: LengthUnit::M }
    }
    pub const fn inches(value: f32) -> Self {
        Self { value, unit: LengthUnit::In }
    }

    /// Convert to the kernel's native model unit (millimetres).
    pub fn to_mm(&self) -> f32 {
        match self.unit {
            LengthUnit::Mm => self.value,
            LengthUnit::Cm => self.value * 10.0,
            LengthUnit::M => self.value * 1000.0,
            LengthUnit::In => self.value * 25.4,
        }
    }
}

/// Angle unit, wire-encoded as a lowercase string (`"deg"`, `"rad"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AngleUnit {
    Deg,
    Rad,
}

/// An angle value with an explicit unit. Wire form is
/// `{"value": 45.0, "unit": "deg"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Angle {
    pub value: f32,
    pub unit: AngleUnit,
}

impl Angle {
    pub const fn deg(value: f32) -> Self {
        Self { value, unit: AngleUnit::Deg }
    }
    pub const fn rad(value: f32) -> Self {
        Self { value, unit: AngleUnit::Rad }
    }

    /// Convert to the kernel's native model unit (radians).
    pub fn to_rad(&self) -> f32 {
        match self.unit {
            AngleUnit::Deg => self.value * std::f32::consts::PI / 180.0,
            AngleUnit::Rad => self.value,
        }
    }
}

/// A geometry-selection query that a feature parameter references.
///
/// Wire form is a bare string — `"qCreatedBy(box_1)"`,
/// `"qFaceOf(extrude_3, top)"`, etc. K4.5.4f only introduces the
/// type so features can declare query parameters; actual
/// evaluation against the contributor graph is K4.5.7's
/// responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Query {
    pub expr: String,
}

impl Query {
    pub fn new(expr: impl Into<String>) -> Self {
        Self { expr: expr.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.expr
    }
}

/// Affine placement of a feature in world space.
///
/// Stored as **translate + unit-quaternion rotation + uniform scale**.
/// Per-axis scale would break the SDF distance property (the field
/// stops being a true Euclidean distance), so we keep scale uniform.
/// Quaternion order is `(x, y, z, w)`; identity is `(0, 0, 0, 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translate: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: f32,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translate: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: 1.0,
    };

    pub fn is_identity(&self) -> bool {
        self.translate == [0.0, 0.0, 0.0]
            && self.rotation == [0.0, 0.0, 0.0, 1.0]
            && self.scale == 1.0
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// One node in the feature spine. At M2 the spine is linear; later
/// milestones (Pattern, Group, Mirror) will introduce nested children
/// that share the same `Feature` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,
    pub primitive: Primitive,
    /// Affine placement of the primitive in world space.
    /// `#[serde(default)]` lets us roll forward older documents that
    /// predate transforms — they load with identity placement.
    #[serde(default)]
    pub transform: Transform,
    /// K10.1: per-junction blend radius in mm. When > 0 the CSG
    /// tree builder (`CsgNode::from_features`) uses
    /// `SmoothUnion`/`SmoothDifference` instead of the hard
    /// variants for this feature's Boolean junction. Zero (the
    /// default) keeps the classic sharp edges.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blend_radius: f32,
    /// K11.1: twist rate in radians per mm around the local Y
    /// axis. When non-zero, `CsgNode::from_features` wraps
    /// this feature's leaf in a `CsgNode::Twist` before folding
    /// it into the accumulator. Zero = no twist.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub twist_rate: f32,
    /// K11.2: bend rate in radians per mm in the XY plane along
    /// the X axis. Same domain-warp approach as twist_rate.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend_rate: f32,
    /// K11.3: displacement depth in mm. When > 0 the CSG builder
    /// wraps the leaf in `CsgNode::Displace`. Zero = smooth surface.
    /// K10.2: chamfer size in mm. When > 0 and `blend_radius` is
    /// 0, `from_features` uses the chamfer blend formula (flat
    /// 45° cut) instead of the fillet. Mutually exclusive with
    /// `blend_radius` at the UX level; if both are set the fillet
    /// wins.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chamfer_size: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub displace_depth: f32,
    /// K11.3: displacement spatial frequency (cycles/mm).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub displace_freq: f32,
    /// Shell thickness in mm. When > 0, from_features wraps the
    /// leaf in CsgNode::Shell, turning the solid into a thin
    /// wall of this thickness.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shell_thickness: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// One entry in the K4.5 Feature-as-Function feature tree.
///
/// The canonical §4.6 JSON wire format for every mutation a user
/// makes to a document: a `{ type, id, params }` triple where the
/// type name picks a [`FeatureKind`] (in the kernel registry) and
/// the `params` blob carries the user's typed parameter values as
/// raw JSON until a `FeatureSchema` derive validates them.
///
/// K4.5.4b moved this struct from `kernel::features` to the
/// protocol crate so it can eventually live inside [`Document`]
/// as the authoritative serialisation form (K4.5.4c+). The kernel
/// re-exports `FeatureCall` from `crate::features::FeatureCall`
/// so existing imports keep working — the move is purely
/// structural, no behaviour changed in this slice.
///
/// [`FeatureKind`]: ../fielddraft_kernel/features/trait.FeatureKind.html
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureCall {
    /// Registry key for the feature type. `#[serde(rename = "type")]`
    /// emits the §4.6 canonical field name on the wire.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Stable id of this call within the feature tree. Step 1
    /// (K4.5.1) introduced the hierarchical [`FeatureId`] that
    /// sub-operations inside a feature body can extend with
    /// child names.
    pub id: FeatureId,
    /// Typed parameter values. Held as raw `serde_json::Value` so
    /// the wire shape is schema-less until the feature's
    /// `FeatureKind::execute` parses it.
    #[serde(default)]
    pub params: serde_json::Value,
    /// K8.7: per-call suppression flag. When `true`, the
    /// regeneration loop skips this call — its geometry drops
    /// out of the CSG tree but the call itself stays in the
    /// document so the user can unsuppress it later. Skipped
    /// from serialization when `false` so existing fixtures
    /// and round-trip tests see a byte-identical wire form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppressed: bool,
}

impl FeatureCall {
    /// Build a feature call from its three canonical fields.
    pub fn new(
        type_name: impl Into<String>,
        id: FeatureId,
        params: serde_json::Value,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            id,
            params,
            suppressed: false,
        }
    }

    /// Lossless conversion from a baked-in [`Feature`] (the K1 spine
    /// entry) into the K4.5 Feature-as-Function wire form.
    ///
    /// K4.5.4c uses this to drive [`Document::sync_feature_calls`]:
    /// after every mutation that touches `document.features`, the
    /// `feature_calls` mirror is rebuilt by calling this conversion
    /// on each feature. *Every* `Feature` field ends up in
    /// `params`, including the full `Transform` (translate,
    /// rotation, scale) and the display `name`, so step 4d can flip
    /// to `feature_calls` as the authoritative form without losing
    /// information that the hand-rolled K4.5.4a primitive derives
    /// don't know about yet (the derives only model the parameters
    /// the UI dialog will eventually touch).
    pub fn from_spine_feature(f: &Feature) -> Self {
        let type_name = match &f.primitive {
            Primitive::Box { .. } => "box",
            Primitive::Sphere { .. } => "sphere",
            Primitive::Cylinder { .. } => "cylinder",
            // K7.3: Primitive::Extrude on the spine is a
            // *derived* artefact — the authoritative wire form
            // is the original "extrude" FeatureCall with its
            // sketch query, preserved verbatim by
            // `sync_feature_calls` via the is_spine_type filter
            // below. Emit a placeholder type_name here so the
            // sync can skip re-adding it.
            Primitive::Gyroid { .. } => "__derived_gyroid_skip__",
            Primitive::Extrude { .. } => "__derived_extrude_skip__",
            // K8.4: same derivation rule as Extrude — the
            // authoritative wire form is the original "revolve"
            // FeatureCall with its sketch query.
            Primitive::Revolve { .. } => "__derived_revolve_skip__",
            Primitive::Torus { .. } => "torus",
            Primitive::HexPrism { .. } => "hex_prism",
            Primitive::Capsule { .. } => "capsule",
            Primitive::Cone { .. } => "cone",
            Primitive::Thread { .. } => "thread",
            // ImportedMesh is a non-spine type (hierarchical child id),
            // emit a skip marker so sync_feature_calls doesn't re-add it.
            Primitive::ImportedMesh { .. } => "__derived_imported_mesh_skip__",
        };
        let op = bool_op_wire_name(f.op);
        let translate = f.transform.translate;
        let rotation = f.transform.rotation;
        let scale = f.transform.scale;
        let name = &f.name;

        let params = match &f.primitive {
            Primitive::Box {
                center,
                half_extents,
            } => serde_json::json!({
                "half_extents": half_extents,
                "center": center,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Sphere { center, radius } => serde_json::json!({
                "radius": radius,
                "center": center,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Cylinder {
                center,
                radius,
                half_height,
            } => serde_json::json!({
                "radius": radius,
                "half_height": half_height,
                "center": center,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Extrude {
                sketch,
                half_depth_z,
                center,
            } => serde_json::json!({
                "sketch": sketch,
                "half_depth_z": half_depth_z,
                "center": center,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Gyroid { cell_size, thickness } => serde_json::json!({
                "cell_size": cell_size,
                "thickness": thickness,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Revolve { sketch, center } => serde_json::json!({
                "sketch": sketch,
                "center": center,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Torus { center, major_radius, minor_radius } => serde_json::json!({
                "center": center,
                "major_radius": major_radius,
                "minor_radius": minor_radius,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::HexPrism { apothem, half_height } => serde_json::json!({
                "apothem": apothem,
                "half_height": half_height,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Capsule { radius, half_height } => serde_json::json!({
                "radius": radius,
                "half_height": half_height,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Cone {
                r_bottom,
                r_top,
                half_height,
            } => serde_json::json!({
                "r_bottom": r_bottom,
                "r_top": r_top,
                "half_height": half_height,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::Thread {
                nominal_radius,
                pitch,
                depth,
                half_height,
            } => serde_json::json!({
                "nominal_radius": nominal_radius,
                "pitch": pitch,
                "depth": depth,
                "half_height": half_height,
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
            Primitive::ImportedMesh { mesh } => serde_json::json!({
                "mesh": serde_json::to_value(&**mesh).unwrap_or_default(),
                "translate": translate,
                "rotation": rotation,
                "scale": scale,
                "op": op,
                "name": name,
            }),
        };

        Self {
            type_name: type_name.to_string(),
            id: f.id.clone(),
            params,
            suppressed: false,
        }
    }
}

/// Wire-format name for a boolean operation. Mirrors the
/// `#[serde(rename_all = "snake_case")]` on [`BoolOp`] so the K4.5
/// `FeatureCall::params.op` field uses the same spelling as the
/// top-level `Feature.op` field does in the legacy K1 JSON.
fn bool_op_wire_name(op: BoolOp) -> &'static str {
    match op {
        BoolOp::Union => "union",
        BoolOp::Difference => "difference",
        BoolOp::Intersect => "intersect",
    }
}

/// Project document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    /// Linear feature list, evaluated top-to-bottom. Stays the
    /// authoritative in-memory working storage the mesher walks
    /// and the kernel mutates; K4.5.4e strips it from the JSON
    /// wire form via `skip_serializing`, so save/load now flow
    /// exclusively through [`Document::feature_calls`]. The
    /// `default` lets legacy K1 JSON that still carries a
    /// top-level `features` array deserialise cleanly — those
    /// documents never hit the bootstrap path because their
    /// `feature_calls` list is empty, and `sync_feature_calls`
    /// repopulates it on the next mutation.
    #[serde(skip_serializing, default)]
    pub features: Vec<Feature>,
    /// K4.5 Feature-as-Function canonical wire form.
    ///
    /// K4.5.4e promotes this to the **only** feature list that
    /// round-trips through JSON. On serialize, `sync_feature_calls`
    /// keeps it in lock-step with the in-memory
    /// [`Document::features`] spine after every mutation. On
    /// deserialize, the kernel detects a populated `feature_calls`
    /// with empty `features` and rehydrates the spine via
    /// `FeatureRegistry::regenerate` (K4.5.4d bootstrap). Legacy
    /// K1 documents without this field still load — the
    /// `default` gives them an empty list, and the loader falls
    /// through the bootstrap branch because `features` is
    /// already populated.
    #[serde(default)]
    pub feature_calls: Vec<FeatureCall>,
    /// Monotonic counter that seeds the next flat-numeric
    /// [`FeatureId`]. Persisted so reloads keep stable ids. The
    /// field name stays as `next_feature_id` for JSON backward
    /// compatibility with K1 documents that wrote it as a bare
    /// integer; the type is a plain counter because K4.5.1 still
    /// only issues flat-numeric ids. A future K4.5 slice will
    /// grow a proper hierarchical allocator on top.
    pub next_feature_id: u32,
    /// K8.10: document-level scalar variables. Keyed by
    /// user-chosen name, value in the model unit
    /// (millimetres for lengths, dimensionless otherwise).
    /// The K8.11 formula engine will teach Length fields to
    /// reference these by name; this first slice is just the
    /// persisted table. BTreeMap so the wire form is
    /// deterministically ordered across saves — snapshots
    /// diff cleanly.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub variables: std::collections::BTreeMap<String, f32>,
}

impl Default for Document {
    fn default() -> Self {
        // The empty default has no features at all so the user starts
        // with a clean slate. Tests and the smoke E2E both rely on this.
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled".to_string(),
            features: Vec::new(),
            feature_calls: Vec::new(),
            next_feature_id: 1,
            variables: std::collections::BTreeMap::new(),
        }
    }
}

impl Document {
    /// Allocate the next flat-numeric [`FeatureId`] and bump the
    /// counter. K4.5.1 still uses the K1 allocation scheme (`"1"`,
    /// `"2"`, `"3"` in the new typed form). Hierarchical allocation
    /// via the Feature Registry will come in a later slice.
    pub fn allocate_id(&mut self) -> FeatureId {
        let n = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.saturating_add(1);
        FeatureId::from_u32(n)
    }

    /// Rebuild [`Document::feature_calls`] from the current
    /// [`Document::features`] list via
    /// [`FeatureCall::from_spine_feature`]. Called by the kernel's
    /// `rebuild_packed` after every mutation.
    ///
    /// K6b.3a: the sync now preserves `feature_calls` entries whose
    /// `type_name` is **not** one of the primitive/spine-backed
    /// kinds (`box`, `sphere`, `cylinder`, `extrude`). Those
    /// survive the wipe-and-rebuild and keep their position after
    /// the regenerated spine entries. This is the extension point
    /// that lets sketches live in `feature_calls` without ever
    /// being pushed to the in-memory `features` spine — a sketch
    /// has no `SpineFeature` shape (no primitive, no transform)
    /// and lives purely as a wire-form call whose `execute`
    /// populates `Context::sketches` at regeneration time.
    pub fn sync_feature_calls(&mut self) {
        // K8.7: suppressed calls never run through `execute`, so
        // they have no SpineFeature mirror to rebuild from. Keep
        // them verbatim regardless of type — without this gate a
        // suppressed box would vanish from the wire form on the
        // very next sync.
        let surviving: Vec<FeatureCall> = self
            .feature_calls
            .drain(..)
            .filter(|c| !is_spine_type(&c.type_name) || c.suppressed)
            .collect();
        let mut next: Vec<FeatureCall> = self
            .features
            .iter()
            // K8.0: spine entries with a hierarchical parent
            // id (e.g. `linear_pattern_3/1`) are clones that
            // were produced by their parent feature's execute
            // path. Don't re-emit them as independent wire
            // entries — the parent call is authoritative.
            .filter(|f| f.id.parent().is_none())
            .map(FeatureCall::from_spine_feature)
            // K7.3: Primitive::Extrude spine entries are
            // derived from the original "extrude" FeatureCall
            // at regen time. Don't re-emit them here —
            // `surviving` already preserves the authoritative
            // entry because the is_spine_type filter above
            // doesn't treat them as spine-derived.
            .filter(|call| {
                call.type_name != "__derived_extrude_skip__"
                    && call.type_name != "__derived_revolve_skip__"
                    && call.type_name != "__derived_gyroid_skip__"
                    && call.type_name != "__derived_imported_mesh_skip__"
            })
            .collect();
        next.extend(surviving);
        self.feature_calls = next;
    }
}

/// Feature type names whose `FeatureCall` entries mirror a real
/// [`Feature`] on the document spine. Used by
/// [`Document::sync_feature_calls`] to decide which entries to
/// rebuild from `features` vs. which ones to preserve verbatim.
///
/// K7.3 removes `"extrude"` from this set: extrude calls now
/// survive the sync verbatim (same rule as `"sketch"`) because
/// `Primitive::Extrude` on the spine is a derived artefact —
/// the canonical wire form is the original extrude
/// `FeatureCall` with its sketch query.
fn is_spine_type(type_name: &str) -> bool {
    matches!(type_name, "box" | "sphere" | "cylinder" | "torus" | "thread" | "cone" | "capsule" | "hex_prism")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_is_empty() {
        let doc = Document::default();
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
        assert!(doc.features.is_empty());
        assert_eq!(doc.next_feature_id, 1);
    }

    // ─── Document.feature_calls (K4.5.4c) ──────────────────────

    #[test]
    fn default_document_has_empty_feature_calls() {
        let doc = Document::default();
        assert!(doc.feature_calls.is_empty());
    }

    #[test]
    fn sync_feature_calls_derives_one_entry_per_feature() {
        let mut doc = Document::default();
        let id1 = doc.allocate_id();
        let id2 = doc.allocate_id();
        doc.features.push(Feature {
            id: id1.clone(),
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        doc.features.push(Feature {
            id: id2.clone(),
            name: "Hole".into(),
            op: BoolOp::Difference,
            primitive: Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 3.0,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });

        doc.sync_feature_calls();
        assert_eq!(doc.feature_calls.len(), 2);
        assert_eq!(doc.feature_calls[0].type_name, "box");
        assert_eq!(doc.feature_calls[0].id, id1);
        assert_eq!(doc.feature_calls[1].type_name, "sphere");
        assert_eq!(doc.feature_calls[1].id, id2);
    }

    #[test]
    fn sync_feature_calls_preserves_non_spine_entries() {
        // K6b.3a: a `sketch` FeatureCall lives in `feature_calls`
        // without a matching `features` spine entry. After a
        // sync it must still be there, positioned after the
        // rebuilt primitive entries.
        let mut doc = Document::default();
        let id_box = doc.allocate_id();
        let id_sketch = doc.allocate_id();
        doc.features.push(Feature {
            id: id_box.clone(),
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        doc.feature_calls.push(FeatureCall::new(
            "sketch",
            id_sketch.clone(),
            serde_json::json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [],
            }),
        ));

        doc.sync_feature_calls();

        assert_eq!(doc.feature_calls.len(), 2);
        assert_eq!(doc.feature_calls[0].type_name, "box");
        assert_eq!(doc.feature_calls[0].id, id_box);
        assert_eq!(doc.feature_calls[1].type_name, "sketch");
        assert_eq!(doc.feature_calls[1].id, id_sketch);
    }

    #[test]
    fn from_spine_feature_encodes_every_box_field_in_params() {
        let feature = Feature {
            id: FeatureId::from_u32(1),
            name: "Corner".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [1.0, 2.0, 3.0],
                half_extents: [5.0, 6.0, 7.0],
            },
            transform: Transform {
                translate: [10.0, 20.0, 30.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 2.0,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        };
        let call = FeatureCall::from_spine_feature(&feature);
        assert_eq!(call.type_name, "box");
        assert_eq!(call.id, FeatureId::from_u32(1));
        assert_eq!(
            call.params,
            serde_json::json!({
                "half_extents": [5.0, 6.0, 7.0],
                "center": [1.0, 2.0, 3.0],
                "translate": [10.0, 20.0, 30.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": 2.0,
                "op": "union",
                "name": "Corner"
            })
        );
    }

    #[test]
    fn from_spine_feature_encodes_sphere_and_cylinder() {
        let sphere = Feature {
            id: FeatureId::from_u32(1),
            name: "Ball".into(),
            op: BoolOp::Difference,
            primitive: Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 4.0,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        };
        let call = FeatureCall::from_spine_feature(&sphere);
        assert_eq!(call.type_name, "sphere");
        assert_eq!(call.params["radius"], serde_json::json!(4.0));
        assert_eq!(call.params["op"], serde_json::json!("difference"));

        let cylinder = Feature {
            id: FeatureId::from_u32(2),
            name: "Pin".into(),
            op: BoolOp::Intersect,
            primitive: Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: 1.5,
                half_height: 10.0,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        };
        let call = FeatureCall::from_spine_feature(&cylinder);
        assert_eq!(call.type_name, "cylinder");
        assert_eq!(call.params["radius"], serde_json::json!(1.5));
        assert_eq!(call.params["half_height"], serde_json::json!(10.0));
        assert_eq!(call.params["op"], serde_json::json!("intersect"));
    }

    #[test]
    fn document_round_trips_feature_calls_through_json() {
        // K4.5.4e: the only feature list that survives a JSON
        // round-trip is `feature_calls`. `features` is
        // `skip_serializing`, so the parsed document comes back
        // with an empty spine — the kernel bootstraps it from
        // `feature_calls` in `load_json`. At the protocol level
        // we assert only the wire-carried fields.
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.features.push(Feature {
            id: id.clone(),
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        doc.sync_feature_calls();

        let wire = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed.schema_version, doc.schema_version);
        assert_eq!(parsed.name, doc.name);
        assert_eq!(parsed.next_feature_id, doc.next_feature_id);
        assert!(parsed.features.is_empty());
        assert_eq!(parsed.feature_calls, doc.feature_calls);
        assert_eq!(parsed.feature_calls.len(), 1);
        assert_eq!(parsed.feature_calls[0].id, id);
    }

    #[test]
    fn document_json_wire_form_omits_features_field() {
        // K4.5.4e: serialize a populated document and assert the
        // wire-form JSON contains `feature_calls` but no
        // `features` key at the top level.
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.features.push(Feature {
            id,
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        doc.sync_feature_calls();

        let wire = serde_json::to_value(&doc).unwrap();
        let obj = wire.as_object().expect("document serialises to object");
        assert!(
            !obj.contains_key("features"),
            "K4.5.4e must not emit the features field: {wire}"
        );
        assert!(
            obj.contains_key("feature_calls"),
            "K4.5.4e must emit feature_calls: {wire}"
        );
    }

    #[test]
    fn legacy_k1_document_without_feature_calls_loads_with_empty_list() {
        // K1-era documents predate the K4.5.4c dual-write field.
        // The `#[serde(default)]` attribute on feature_calls lets
        // them deserialise cleanly with an empty list — the next
        // kernel mutation will fill it in via sync_feature_calls.
        let json = r#"{
            "schema_version": 2,
            "name": "legacy",
            "features": [{
                "id": 1,
                "name": "Base",
                "op": "union",
                "primitive": {
                    "kind": "box",
                    "center": [0, 0, 0],
                    "half_extents": [1, 1, 1]
                },
                "transform": {
                    "translate": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": 1
                }
            }],
            "next_feature_id": 2
        }"#;
        let parsed: Document = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.features.len(), 1);
        assert!(parsed.feature_calls.is_empty());
    }

    #[test]
    fn document_roundtrips_through_json() {
        // K4.5.4e: the top-level `features` array no longer
        // survives serialize; the kernel carries the bootstrap.
        // At the protocol level we verify that syncing first,
        // then round-tripping, leaves all wire-carried fields
        // intact.
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.features.push(Feature {
            id,
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        doc.sync_feature_calls();

        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, doc.schema_version);
        assert_eq!(parsed.name, doc.name);
        assert_eq!(parsed.next_feature_id, doc.next_feature_id);
        assert_eq!(parsed.feature_calls, doc.feature_calls);
        assert!(parsed.features.is_empty());
    }

    #[test]
    fn transform_default_is_identity() {
        let t = Transform::default();
        assert!(t.is_identity());
        assert_eq!(t.translate, [0.0, 0.0, 0.0]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.scale, 1.0);
    }

    #[test]
    fn legacy_feature_without_transform_loads_with_identity() {
        // A document that predates the transform field should still parse,
        // gaining `Transform::IDENTITY` via `#[serde(default)]`.
        let json = r#"{
            "schema_version": 2,
            "name": "legacy",
            "features": [{
                "id": 1,
                "name": "Base",
                "op": "union",
                "primitive": {"kind":"box","center":[0,0,0],"half_extents":[1,1,1]}
            }],
            "next_feature_id": 2
        }"#;
        let parsed: Document = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.features.len(), 1);
        assert!(parsed.features[0].transform.is_identity());
    }

    #[test]
    fn allocate_id_is_monotonic() {
        let mut doc = Document::default();
        let a = doc.allocate_id();
        let b = doc.allocate_id();
        let c = doc.allocate_id();
        // K4.5 still issues flat-numeric ids, so we can round-trip
        // them through u32 for the ordering assertion.
        let an = a.as_u32_if_flat().unwrap();
        let bn = b.as_u32_if_flat().unwrap();
        let cn = c.as_u32_if_flat().unwrap();
        assert!(an < bn && bn < cn);
        assert_eq!(doc.next_feature_id, cn + 1);
    }

    #[test]
    fn feature_id_display_and_parse_round_trip() {
        let flat = FeatureId::from_u32(42);
        assert_eq!(flat.to_string(), "42");
        assert_eq!(FeatureId::from_str("42").unwrap(), flat);
        assert!(flat.is_flat_numeric());

        let child = flat.child("op");
        assert_eq!(child.to_string(), "42.op");
        assert_eq!(FeatureId::from_str("42.op").unwrap(), child);
        assert!(!child.is_flat_numeric());
        assert!(child.as_u32_if_flat().is_none());

        let grand = child.child("mask_0");
        assert_eq!(grand.to_string(), "42.op.mask_0");
        assert_eq!(FeatureId::from_str("42.op.mask_0").unwrap(), grand);
    }

    #[test]
    fn feature_id_serializes_flat_numeric_as_bare_integer() {
        // Backward-compat with K1 JSON documents that stored ids
        // as bare integers.
        let id = FeatureId::from_u32(7);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "7");
        let parsed: FeatureId = serde_json::from_str("7").unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn feature_id_serializes_hierarchical_as_string() {
        let id = FeatureId::from_u32(3).child("op");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"3.op\"");
        let parsed: FeatureId = serde_json::from_str("\"3.op\"").unwrap();
        assert_eq!(parsed, id);
    }

    // ─── FeatureCall (K4.5.4b) ──────────────────────────────────

    #[test]
    fn feature_call_builds_from_fields() {
        let call = FeatureCall::new(
            "box",
            FeatureId::from_u32(7),
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] }),
        );
        assert_eq!(call.type_name, "box");
        assert_eq!(call.id, FeatureId::from_u32(7));
        assert_eq!(
            call.params,
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] })
        );
    }

    #[test]
    fn feature_call_serde_shape_uses_type_field_not_type_name() {
        // §4.6 canonical wire shape: `{ "type": "...", "id": ...,
        // "params": {...} }`. The Rust field is called `type_name`
        // because `type` is a reserved keyword; the
        // `#[serde(rename)]` glue emits `type` on the wire.
        let call = FeatureCall::new(
            "sphere",
            FeatureId::from_u32(3),
            serde_json::json!({ "radius": 5.0 }),
        );
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire.get("type"), Some(&serde_json::json!("sphere")));
        assert!(wire.get("type_name").is_none());
        assert_eq!(wire.get("id"), Some(&serde_json::json!(3)));
        assert_eq!(
            wire.get("params"),
            Some(&serde_json::json!({ "radius": 5.0 }))
        );
    }

    #[test]
    fn feature_call_round_trips_through_json() {
        let call = FeatureCall::new(
            "cylinder",
            FeatureId::from_u32(1).child("cap"),
            serde_json::json!({
                "radius": 4.0,
                "half_height": 10.0,
                "op": "difference"
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let parsed: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed, call);
    }

    #[test]
    fn feature_call_params_default_to_null_when_missing() {
        // `#[serde(default)]` lets a call without a params field
        // still deserialise — handy for the K4.5 step-5 JS shell
        // that sometimes emits parameter-less calls during drag
        // previews.
        let wire = r#"{"type":"box","id":2}"#;
        let parsed: FeatureCall = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.type_name, "box");
        assert_eq!(parsed.id, FeatureId::from_u32(2));
        assert_eq!(parsed.params, serde_json::Value::Null);
    }

    #[test]
    fn feature_id_deserializer_accepts_both_shapes() {
        let from_int: FeatureId = serde_json::from_str("5").unwrap();
        assert_eq!(from_int, FeatureId::from_u32(5));
        let from_str: FeatureId = serde_json::from_str("\"5\"").unwrap();
        assert_eq!(from_str, FeatureId::from_u32(5));
    }

    #[test]
    fn primitive_kind_tags_match_constants() {
        assert_eq!(
            Primitive::Box { center: [0.0; 3], half_extents: [1.0; 3] }.kind_tag(),
            Primitive::KIND_BOX
        );
        assert_eq!(
            Primitive::Sphere { center: [0.0; 3], radius: 1.0 }.kind_tag(),
            Primitive::KIND_SPHERE
        );
        assert_eq!(
            Primitive::Cylinder { center: [0.0; 3], radius: 1.0, half_height: 1.0 }.kind_tag(),
            Primitive::KIND_CYLINDER
        );
    }

    #[test]
    fn bool_op_serializes_snake_case() {
        let json = serde_json::to_string(&BoolOp::Difference).unwrap();
        assert_eq!(json, "\"difference\"");
        let parsed: BoolOp = serde_json::from_str("\"intersect\"").unwrap();
        assert_eq!(parsed, BoolOp::Intersect);
    }

    #[test]
    fn bool_op_tags_match_constants() {
        assert_eq!(BoolOp::Union.tag(), BoolOp::TAG_UNION);
        assert_eq!(BoolOp::Difference.tag(), BoolOp::TAG_DIFFERENCE);
        assert_eq!(BoolOp::Intersect.tag(), BoolOp::TAG_INTERSECT);
    }

    // ─── K4.5.4f: Length / Angle / Query runtime types ────────

    #[test]
    fn length_wire_form_is_value_plus_unit() {
        let l = Length::mm(12.5);
        let wire = serde_json::to_value(&l).unwrap();
        assert_eq!(wire, serde_json::json!({"value": 12.5, "unit": "mm"}));
        let parsed: Length =
            serde_json::from_value(serde_json::json!({"value": 5.0, "unit": "cm"}))
                .unwrap();
        assert_eq!(parsed, Length::cm(5.0));
    }

    #[test]
    fn length_to_mm_converts_every_unit() {
        assert_eq!(Length::mm(10.0).to_mm(), 10.0);
        assert_eq!(Length::cm(1.0).to_mm(), 10.0);
        assert_eq!(Length::m(2.5).to_mm(), 2500.0);
        assert!((Length::inches(1.0).to_mm() - 25.4).abs() < 1e-4);
    }

    #[test]
    fn length_rejects_unknown_unit() {
        let err = serde_json::from_value::<Length>(
            serde_json::json!({"value": 1.0, "unit": "furlong"}),
        );
        assert!(err.is_err());
    }

    #[test]
    fn angle_wire_form_is_value_plus_unit() {
        let a = Angle::deg(90.0);
        let wire = serde_json::to_value(&a).unwrap();
        assert_eq!(wire, serde_json::json!({"value": 90.0, "unit": "deg"}));
        let parsed: Angle = serde_json::from_value(
            serde_json::json!({"value": 1.5, "unit": "rad"}),
        )
        .unwrap();
        assert_eq!(parsed, Angle::rad(1.5));
    }

    #[test]
    fn angle_to_rad_converts_both_units() {
        assert!((Angle::deg(180.0).to_rad() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(Angle::rad(1.23).to_rad(), 1.23);
    }

    #[test]
    fn query_wire_form_is_bare_string() {
        let q = Query::new("qCreatedBy(box_1)");
        let wire = serde_json::to_value(&q).unwrap();
        assert_eq!(wire, serde_json::json!("qCreatedBy(box_1)"));
        let parsed: Query =
            serde_json::from_value(serde_json::json!("qFaceOf(extrude_3, top)"))
                .unwrap();
        assert_eq!(parsed.as_str(), "qFaceOf(extrude_3, top)");
    }
}
