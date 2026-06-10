//! Proc-macros for the FieldDraft feature system (§4.6, K4.5 step 3).
//!
//! This crate sits beside `fielddraft-kernel` in the workspace and
//! exports a single derive:
//!
//! ```ignore
//! use fielddraft_features_macros::FeatureSchema;
//!
//! #[derive(FeatureSchema)]
//! #[schema(type = "my_feature")]
//! pub struct MyDef {
//!     pub depth: f32,
//!     pub origin: [f32; 3],
//!     pub enabled: bool,
//!     pub label: String,
//! }
//! ```
//!
//! The struct-level attribute is called `#[schema(...)]` rather
//! than `#[feature(...)]` to sidestep the collision with Rust's
//! built-in (nightly) `#[feature(...)]` gate attribute, which
//! otherwise produces an "ambiguous attribute name" error when
//! the derive and the gate live in the same translation unit.
//!
//! After the derive, three inherent methods live on `MyDef`:
//!
//! * `MyDef::type_name() -> &'static str` — returns `"my_feature"`.
//! * `MyDef::schema_json() -> serde_json::Value` — a small FieldDraft
//!   schema shape (§4.6 wire format) that the K6a dialog generator
//!   will consume once it lands.
//! * `MyDef::parse(def: &serde_json::Value) -> Result<Self,
//!   fielddraft_kernel::features::FeatureError>` — validates the
//!   definition JSON and extracts typed field values.
//!
//! ## Supported scope (K4.5.4f)
//!
//! * **Field types:** `f32`, `[f32; 3]`, `bool`, `String`, `BoolOp`
//!   (K4.5.4a), and — as of K4.5.4f — the typed runtime
//!   parameters `Length`, `Angle`, and `Query` from
//!   `fielddraft_protocol`. Any other type path falls through to a
//!   generic `serde_json::from_value` dispatch, which means user-
//!   defined enums (and other `Serialize + Deserialize` types) can
//!   be used as feature parameters without touching this derive.
//!   `Option<T>` still comes later.
//! * **Struct-level attribute:** `#[schema(type = "…")]` sets the
//!   type name that goes into the registry and the on-wire JSON.
//! * **Field-level `#[param(...)]` attribute:**
//!   * `name = "Pretty Name"` overrides the display name (default
//!     is the prettified Rust identifier, `half_extents` →
//!     `Half extents`).
//!   * `default = <expr>` makes the field optional and uses the
//!     Rust expression as the fallback when the definition JSON
//!     is missing the key or sends `null`. The expression is
//!     quoted verbatim, so anything that's a valid Rust rvalue
//!     works: `0.0`, `[0.0, 0.0, 0.0]`, `BoolOp::New`, etc.
//!   * `ui_hint = "horizontal_enum" | "opposite_direction" |
//!     "slider"` — rendering hint forwarded verbatim into the
//!     schema JSON. The TS FeatureDialog (K6a.1) dispatches
//!     widgets on it: `horizontal_enum` selects a segmented-tab
//!     enum, `opposite_direction` turns a boolean into an inline
//!     ±-flip button, `slider` turns an `f32` into a range slider.
//!     The `bounds` / `visible_when` / `filter` keys from the §4.6
//!     vocabulary are still emitted on the schema side but come
//!     from hand-written fixtures rather than the derive for now.
//! * **No `impl FeatureKind`.** The derive only emits schema /
//!   parse helpers. Callers still write `FeatureKind` by hand,
//!   just shorter — the execute body opens with
//!   `let def = MyDef::parse(def)?;` instead of reaching into
//!   the JSON directly.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{
    parse_macro_input, spanned::Spanned, Data, DeriveInput, Expr, ExprLit,
    Fields, Lit, Type, TypeArray, TypePath,
};

/// Derive macro for `FeatureSchema`. See the crate docs for the
/// full input-shape and output contract. Honours `#[schema(type =
/// "…")]` at the struct level and `#[param(name = "…", default =
/// <expr>)]` on individual fields.
#[proc_macro_derive(FeatureSchema, attributes(schema, param))]
pub fn derive_feature_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

// ─── expansion ──────────────────────────────────────────────────

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let type_name = parse_schema_attr(input)?;

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    input.span(),
                    "FeatureSchema requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "FeatureSchema can only be derived on structs",
            ));
        }
    };

    let mut params: Vec<ParamInfo> = Vec::with_capacity(fields.len());
    for field in fields {
        params.push(ParamInfo::from_field(field)?);
    }

    let schema_entries = params.iter().map(|p| p.emit_schema_entry());
    let parse_binds = params.iter().map(|p| p.emit_parse_binding());
    let field_idents: Vec<_> = params.iter().map(|p| &p.ident).collect();

    Ok(quote! {
        impl #struct_name {
            pub const TYPE_NAME: &'static str = #type_name;

            pub fn type_name() -> &'static str {
                Self::TYPE_NAME
            }

            pub fn schema_json() -> ::serde_json::Value {
                ::serde_json::json!({
                    "type": #type_name,
                    "params": [ #(#schema_entries),* ]
                })
            }

            pub fn parse(
                def: &::serde_json::Value,
            ) -> ::core::result::Result<
                Self,
                crate::features::FeatureError,
            > {
                #(#parse_binds)*
                ::core::result::Result::Ok(Self { #(#field_idents),* })
            }
        }
    })
}

// ─── attribute parsing ──────────────────────────────────────────

/// Pull out the struct-level `#[schema(type = "...")]`. Called
/// `schema`, not `feature`, to avoid colliding with Rust's
/// built-in `#[feature(...)]` attribute (the nightly-only language
/// gate): a derive helper attribute whose name shadows a built-in
/// is reported as "ambiguous attribute name" by rustc.
fn parse_schema_attr(input: &DeriveInput) -> syn::Result<String> {
    for attr in &input.attrs {
        if !attr.path().is_ident("schema") {
            continue;
        }
        let mut type_name: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("type") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                type_name = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("unknown #[schema(...)] key"))
            }
        })?;
        return type_name.ok_or_else(|| {
            syn::Error::new(
                attr.span(),
                "#[schema(...)] must include `type = \"…\"`",
            )
        });
    }
    Err(syn::Error::new(
        input.span(),
        "missing `#[schema(type = \"…\")]` on the struct",
    ))
}

// ─── parameter metadata ─────────────────────────────────────────

#[derive(Clone)]
enum ParamType {
    F32,
    Vec2,
    Vec3,
    Bool,
    String,
    BoolOp,
    /// K4.5.4f: `fielddraft_protocol::Length`.
    Length,
    /// K4.5.4f: `fielddraft_protocol::Angle`.
    Angle,
    /// K4.5.4f: `fielddraft_protocol::Query`.
    Query,
    /// Comma-separated list of query expressions (set semantics).
    MultiQuery,
    /// K4.5.4f: fallback for any other type. The derive emits a
    /// generic `serde_json::from_value::<T>(...)` parse call using
    /// the field's declared type verbatim, which lets user-defined
    /// `Deserialize` types (notably custom enums — the "Enum"
    /// vocabulary slot in the K4.5 roadmap) flow through the
    /// feature schema without any macro changes. Schema emission
    /// reports `"generic"` as the type hint.
    Generic(Box<Type>),
}

impl ParamType {
    fn json_type_name(&self) -> &'static str {
        match self {
            ParamType::F32 => "f32",
            ParamType::Vec2 => "vec2",
            ParamType::Vec3 => "vec3",
            ParamType::Bool => "bool",
            ParamType::String => "string",
            // K6a.1: `BoolOp` is the first kernel-side typed enum
            // that flows through the generic `"enum"` vocabulary
            // slot — the schema payload carries the variant list
            // so the TS dialog can render tabs or a dropdown
            // without knowing about Rust types.
            ParamType::BoolOp => "enum",
            ParamType::Length => "length",
            ParamType::Angle => "angle",
            ParamType::Query => "query",
            ParamType::MultiQuery => "multi_query",
            ParamType::Generic(_) => "generic",
        }
    }

    /// Rendering hint that the derive emits by default for this
    /// type. Overridden by an explicit `#[param(ui_hint = "…")]`
    /// attribute. Today only `BoolOp` opts in (so NAIO still
    /// renders as tabs out of the box); `None` means "leave the
    /// key off the schema entirely".
    fn default_ui_hint(&self) -> Option<&'static str> {
        match self {
            ParamType::BoolOp => Some("horizontal_enum"),
            _ => None,
        }
    }

    /// Variant list for typed enums. Emitted into the schema
    /// entry next to `type: "enum"` so the TS dialog can build
    /// the widget without a separate catalogue call.
    fn variants_json(&self) -> Option<TokenStream2> {
        match self {
            ParamType::BoolOp => Some(quote! {
                ::serde_json::json!([
                    { "key": "add",       "label": "Add" },
                    { "key": "new",       "label": "New" },
                    { "key": "remove",    "label": "Remove" },
                    { "key": "intersect", "label": "Intersect" },
                ])
            }),
            _ => None,
        }
    }
}

struct ParamInfo {
    ident: syn::Ident,
    key: String,
    display_name: String,
    ty: ParamType,
    /// `Some` if the field has a `#[param(default = <expr>)]`
    /// attribute. The field becomes optional in the JSON wire
    /// format, and the generated parse code uses the expression
    /// as the fallback when the key is missing or `null`.
    default: Option<Expr>,
    /// Optional `#[param(ui_hint = "…")]` rendering hint,
    /// forwarded verbatim into the schema JSON so the K6a.1
    /// FeatureDialog can dispatch the right widget without the
    /// Rust side knowing about React.
    ui_hint: Option<String>,
    /// Optional `#[param(unit = "mm")]` display-unit string,
    /// forwarded into the schema JSON so the TS widgets can
    /// show a suffix inside the input field.
    unit: Option<String>,
    /// Optional `#[param(entity_filter = "FACE")]` topology filter,
    /// forwarded into the schema JSON as `entityFilter` so the
    /// viewport element-picker narrows to the right entity kind
    /// (Brocken B/C — e.g. a sketch plane or mate-connector face).
    entity_filter: Option<String>,
}

impl ParamInfo {
    fn from_field(field: &syn::Field) -> syn::Result<Self> {
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(field.span(), "unnamed field"))?;
        let key = ident.to_string();
        let mut display_name = prettify_identifier(&key);
        let mut default: Option<Expr> = None;
        let mut ui_hint: Option<String> = None;
        let mut unit: Option<String> = None;
        let mut entity_filter: Option<String> = None;

        for attr in &field.attrs {
            if !attr.path().is_ident("param") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    display_name = lit.value();
                    Ok(())
                } else if meta.path.is_ident("default") {
                    let value = meta.value()?;
                    let expr: Expr = value.parse()?;
                    default = Some(expr);
                    Ok(())
                } else if meta.path.is_ident("ui_hint") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    ui_hint = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("unit") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    unit = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("entity_filter") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    entity_filter = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown #[param(...)] key \
                         (supported: name, default, ui_hint, unit, entity_filter)",
                    ))
                }
            })?;
        }

        let ty = ParamType::from_type(&field.ty)?;
        Ok(ParamInfo {
            ident,
            key,
            display_name,
            ty,
            default,
            ui_hint,
            unit,
            entity_filter,
        })
    }

    fn emit_schema_entry(&self) -> TokenStream2 {
        let key = &self.key;
        let name = &self.display_name;
        let type_name = self.ty.json_type_name();
        let required = self.default.is_none();

        // Default ui_hint. BoolOp (and any future enum that the
        // derive hardcodes) gets `"horizontal_enum"` so the TS
        // dialog renders it as NAIO-style tabs by default. The
        // `#[param(ui_hint = "…")]` attribute overrides that.
        let effective_hint: Option<String> = match &self.ui_hint {
            Some(h) => Some(h.clone()),
            None => self.ty.default_ui_hint().map(|s| s.to_string()),
        };
        let hint_entry = match effective_hint {
            Some(h) => quote! { "ui_hint": #h, },
            None => quote! {},
        };

        // Typed enums emit their variants alongside `type: "enum"`.
        // Right now only `BoolOp` lands here; once the K6a.1
        // vocabulary grows more kernel-side enums, each gets its
        // own arm in `ParamType::variants_json`.
        let variants_entry = match self.ty.variants_json() {
            Some(tokens) => quote! { "variants": #tokens, },
            None => quote! {},
        };

        let unit_entry = match &self.unit {
            Some(u) => quote! { "unit": #u, },
            None => quote! {},
        };

        let entity_filter_entry = match &self.entity_filter {
            Some(f) => quote! { "entityFilter": #f, },
            None => quote! {},
        };

        // BoolOp defaults differ per feature (primitives seed a New
        // body, extrude-likes Add into the touching one), so the wire
        // key is emitted into the schema; BoolOp is Serialize, json!
        // interpolation lowers the expression to its snake_case name.
        let default_entry = match (&self.ty, &self.default) {
            (ParamType::BoolOp, Some(expr)) => quote! { "default": #expr, },
            _ => quote! {},
        };

        quote! {
            ::serde_json::json!({
                "key": #key,
                "name": #name,
                "type": #type_name,
                "required": #required,
                #default_entry
                #hint_entry
                #variants_entry
                #unit_entry
                #entity_filter_entry
            })
        }
    }

    /// Emit a `let <ident>: <T> = <expr>;` binding that pulls the
    /// parameter out of the definition JSON. When the field has a
    /// `default`, the binding falls back to the Rust expression
    /// on missing / `null`; otherwise missing is a
    /// `MissingParameter` error.
    fn emit_parse_binding(&self) -> TokenStream2 {
        let ident = &self.ident;
        let key = &self.key;
        let rust_ty = self.ty.emit_rust_type();
        let slot_parser = self.ty.emit_slot_parser(key);

        match &self.default {
            None => quote! {
                let #ident: #rust_ty = {
                    let slot = def.get(#key).ok_or(
                        crate::features::FeatureError::MissingParameter {
                            feature: Self::TYPE_NAME,
                            name: #key,
                        },
                    )?;
                    #slot_parser
                };
            },
            Some(default_expr) => quote! {
                let #ident: #rust_ty = match def.get(#key) {
                    ::core::option::Option::None
                    | ::core::option::Option::Some(::serde_json::Value::Null) => {
                        #default_expr
                    }
                    ::core::option::Option::Some(slot) => { #slot_parser }
                };
            },
        }
    }
}

impl ParamType {
    /// Rust type token that the generated `let` binding uses.
    fn emit_rust_type(&self) -> TokenStream2 {
        match self {
            ParamType::F32 => quote! { f32 },
            ParamType::Vec2 => quote! { [f32; 2] },
            ParamType::Vec3 => quote! { [f32; 3] },
            ParamType::Bool => quote! { bool },
            ParamType::String => quote! { ::std::string::String },
            ParamType::BoolOp => quote! { ::fielddraft_protocol::BoolOp },
            ParamType::Length => quote! { ::fielddraft_protocol::Length },
            ParamType::Angle => quote! { ::fielddraft_protocol::Angle },
            ParamType::Query => quote! { ::fielddraft_protocol::Query },
            ParamType::MultiQuery => quote! { ::fielddraft_protocol::MultiQuery },
            ParamType::Generic(ty) => quote! { #ty },
        }
    }

    /// Parse a `&serde_json::Value` named `slot` into the Rust
    /// value of this type. The emitted expression evaluates to
    /// the typed value (via `?` on error).
    fn emit_slot_parser(&self, key: &str) -> TokenStream2 {
        match self {
            ParamType::F32 => quote! {
                slot.as_f64().map(|v| v as f32).ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: "expected a number".to_string(),
                    }
                })?
            },
            ParamType::Vec2 => quote! {{
                let arr = slot.as_array().ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: "expected a 2-element array".to_string(),
                    }
                })?;
                if arr.len() != 2 {
                    return ::core::result::Result::Err(
                        crate::features::FeatureError::InvalidParameter {
                            feature: Self::TYPE_NAME,
                            name: #key,
                            message: ::std::format!(
                                "expected 2 elements, got {}",
                                arr.len()
                            ),
                        },
                    );
                }
                let mut out = [0.0_f32; 2];
                for (i, item) in arr.iter().enumerate() {
                    out[i] = item.as_f64().map(|v| v as f32).ok_or_else(
                        || crate::features::FeatureError::InvalidParameter {
                            feature: Self::TYPE_NAME,
                            name: #key,
                            message: ::std::format!(
                                "element {} is not a number",
                                i
                            ),
                        },
                    )?;
                }
                out
            }},
            ParamType::Vec3 => quote! {{
                let arr = slot.as_array().ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: "expected a 3-element array".to_string(),
                    }
                })?;
                if arr.len() != 3 {
                    return ::core::result::Result::Err(
                        crate::features::FeatureError::InvalidParameter {
                            feature: Self::TYPE_NAME,
                            name: #key,
                            message: ::std::format!(
                                "expected 3 elements, got {}",
                                arr.len()
                            ),
                        },
                    );
                }
                let mut out = [0.0_f32; 3];
                for (i, item) in arr.iter().enumerate() {
                    out[i] = item.as_f64().map(|v| v as f32).ok_or_else(
                        || crate::features::FeatureError::InvalidParameter {
                            feature: Self::TYPE_NAME,
                            name: #key,
                            message: ::std::format!(
                                "element {} is not a number",
                                i
                            ),
                        },
                    )?;
                }
                out
            }},
            ParamType::Bool => quote! {
                slot.as_bool().ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: "expected a boolean".to_string(),
                    }
                })?
            },
            ParamType::String => quote! {
                slot.as_str().map(|s| s.to_string()).ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: "expected a string".to_string(),
                    }
                })?
            },
            ParamType::BoolOp => quote! {{
                let s = slot.as_str().ok_or_else(|| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message:
                            "expected \"add\", \"new\", \"remove\", or \"intersect\""
                                .to_string(),
                    }
                })?;
                match s {
                    "add" => ::fielddraft_protocol::BoolOp::Add,
                    "new" => ::fielddraft_protocol::BoolOp::New,
                    "remove" => ::fielddraft_protocol::BoolOp::Remove,
                    "intersect" => ::fielddraft_protocol::BoolOp::Intersect,
                    other => {
                        return ::core::result::Result::Err(
                            crate::features::FeatureError::InvalidOp {
                                feature: Self::TYPE_NAME,
                                value: other.to_string(),
                            },
                        );
                    }
                }
            }},
            // The Length/Angle/Query/Generic arms all route through
            // serde — each target type implements `Deserialize` and
            // `from_value` gives us typed parsing plus a single
            // unified error path. We clone the slot because
            // `from_value` takes an owned `Value`; the feature
            // definition JSON is typically small, so a shallow
            // clone per parameter is cheap.
            ParamType::Length => quote! {{
                ::serde_json::from_value::<::fielddraft_protocol::Length>(
                    slot.clone(),
                )
                .map_err(|e| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: ::std::format!(
                            "expected a Length ({{\"value\": …, \"unit\": …}}): {e}"
                        ),
                    }
                })?
            }},
            ParamType::Angle => quote! {{
                ::serde_json::from_value::<::fielddraft_protocol::Angle>(
                    slot.clone(),
                )
                .map_err(|e| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: ::std::format!(
                            "expected an Angle ({{\"value\": …, \"unit\": …}}): {e}"
                        ),
                    }
                })?
            }},
            ParamType::Query => quote! {{
                ::serde_json::from_value::<::fielddraft_protocol::Query>(
                    slot.clone(),
                )
                .map_err(|e| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: ::std::format!(
                            "expected a Query string: {e}"
                        ),
                    }
                })?
            }},
            ParamType::MultiQuery => quote! {{
                ::serde_json::from_value::<::fielddraft_protocol::MultiQuery>(
                    slot.clone(),
                )
                .map_err(|e| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: ::std::format!(
                            "expected a MultiQuery string: {e}"
                        ),
                    }
                })?
            }},
            ParamType::Generic(ty) => quote! {{
                ::serde_json::from_value::<#ty>(slot.clone()).map_err(|e| {
                    crate::features::FeatureError::InvalidParameter {
                        feature: Self::TYPE_NAME,
                        name: #key,
                        message: ::std::format!(
                            "failed to deserialise: {e}"
                        ),
                    }
                })?
            }},
        }
    }
}

impl ParamType {
    fn from_type(ty: &Type) -> syn::Result<Self> {
        if let Type::Path(TypePath { qself: None, path }) = ty {
            if let Some(seg) = path.segments.last() {
                match seg.ident.to_string().as_str() {
                    "f32" => return Ok(ParamType::F32),
                    "bool" => return Ok(ParamType::Bool),
                    "String" => return Ok(ParamType::String),
                    // The typed runtime parameter types from
                    // `fielddraft_protocol` are matched by trailing
                    // identifier, so feature definitions can write
                    // the short form (with a `use` in scope) or the
                    // fully qualified path. Both resolve through
                    // the final segment.
                    "BoolOp" => return Ok(ParamType::BoolOp),
                    "Length" => return Ok(ParamType::Length),
                    "Angle" => return Ok(ParamType::Angle),
                    "Query" => return Ok(ParamType::Query),
                    "MultiQuery" => return Ok(ParamType::MultiQuery),
                    _ => {}
                }
            }
            // K4.5.4f: fallback for any other type path. The
            // generated parse code dispatches to
            // `serde_json::from_value::<T>(...)`, which works for
            // any `Deserialize` type — notably user-defined enums,
            // which are the "Enum" slot of the K4.5 feature-type
            // roadmap. The derive never sees the type definition,
            // only the path, so it can't check bounds; callers who
            // need stricter validation still hand-write their
            // parse path.
            return Ok(ParamType::Generic(Box::new(ty.clone())));
        }
        if let Type::Array(TypeArray { elem, len, .. }) = ty {
            // Recognise `[f32; 2]` (sketch-plane coordinates) and
            // `[f32; 3]` (world-space vectors). Other array shapes
            // fall through to the error below.
            if let Type::Path(TypePath { qself: None, path }) = &**elem {
                let is_f32 = path
                    .segments
                    .last()
                    .map(|s| s.ident == "f32")
                    .unwrap_or(false);
                let int_len = if let Expr::Lit(ExprLit {
                    lit: Lit::Int(lit),
                    ..
                }) = len
                {
                    Some(lit.base10_digits())
                } else {
                    None
                };
                if is_f32 {
                    match int_len {
                        Some("2") => return Ok(ParamType::Vec2),
                        Some("3") => return Ok(ParamType::Vec3),
                        _ => {}
                    }
                }
            }
        }
        Err(syn::Error::new(
            ty.span(),
            format!(
                "FeatureSchema does not support field type `{}`. \
                 Named path types (including custom enums) fall back \
                 to a generic `serde_json::from_value` parse; other \
                 shapes such as `[f32; 3]` are recognised explicitly.",
                ty.to_token_stream()
            ),
        ))
    }
}

/// Turn a snake_case identifier into a prettified display name:
/// `half_extents` → `Half extents`.
fn prettify_identifier(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut first = true;
    for word in snake.split('_') {
        if word.is_empty() {
            continue;
        }
        if !first {
            out.push(' ');
            out.push_str(word);
        } else {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_uppercase());
                out.push_str(chars.as_str());
            }
            first = false;
        }
    }
    out
}

