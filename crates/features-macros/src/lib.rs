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
//! ## Supported scope (K4.5.4a)
//!
//! * **Field types:** `f32`, `[f32; 3]`, `bool`, `String`, and
//!   `BoolOp` (the protocol boolean-op enum, parsed from
//!   `"union"`/`"difference"`/`"intersect"`). `Length` / `Angle` /
//!   `Query` / `Option<T>` come in later steps when the kernel
//!   grows the corresponding runtime types.
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
//!     works: `0.0`, `[0.0, 0.0, 0.0]`, `BoolOp::Union`, etc.
//!   The `bounds` / `visible_when` / `filter` / `ui_hint` keys
//!   from the §4.6 vocabulary are not honoured yet.
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

#[derive(Clone, Copy, PartialEq)]
enum ParamType {
    F32,
    Vec3,
    Bool,
    String,
    BoolOp,
}

impl ParamType {
    fn json_type_name(self) -> &'static str {
        match self {
            ParamType::F32 => "f32",
            ParamType::Vec3 => "vec3",
            ParamType::Bool => "bool",
            ParamType::String => "string",
            ParamType::BoolOp => "bool_op",
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
                } else {
                    Err(meta.error(
                        "unknown #[param(...)] key (supported: name, default)",
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
        })
    }

    fn emit_schema_entry(&self) -> TokenStream2 {
        let key = &self.key;
        let name = &self.display_name;
        let type_name = self.ty.json_type_name();
        let required = self.default.is_none();
        quote! {
            ::serde_json::json!({
                "key": #key,
                "name": #name,
                "type": #type_name,
                "required": #required,
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
    fn emit_rust_type(self) -> TokenStream2 {
        match self {
            ParamType::F32 => quote! { f32 },
            ParamType::Vec3 => quote! { [f32; 3] },
            ParamType::Bool => quote! { bool },
            ParamType::String => quote! { ::std::string::String },
            ParamType::BoolOp => quote! { ::fielddraft_protocol::BoolOp },
        }
    }

    /// Parse a `&serde_json::Value` named `slot` into the Rust
    /// value of this type. The emitted expression evaluates to
    /// the typed value (via `?` on error).
    fn emit_slot_parser(self, key: &str) -> TokenStream2 {
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
                            "expected \"union\", \"difference\", or \"intersect\""
                                .to_string(),
                    }
                })?;
                match s {
                    "union" => ::fielddraft_protocol::BoolOp::Union,
                    "difference" => ::fielddraft_protocol::BoolOp::Difference,
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
                    // `BoolOp` is matched by trailing identifier so
                    // the field type can be written as `BoolOp` (with
                    // a `use` in scope) or
                    // `fielddraft_protocol::BoolOp` (fully
                    // qualified). Both resolve through the final
                    // segment.
                    "BoolOp" => return Ok(ParamType::BoolOp),
                    _ => {}
                }
            }
        }
        if let Type::Array(TypeArray { elem, len, .. }) = ty {
            // We only recognise `[f32; 3]` for now.
            if let Type::Path(TypePath { qself: None, path }) = &**elem {
                let is_f32 = path
                    .segments
                    .last()
                    .map(|s| s.ident == "f32")
                    .unwrap_or(false);
                let len_is_three = matches!(
                    len,
                    Expr::Lit(ExprLit {
                        lit: Lit::Int(lit),
                        ..
                    }) if lit.base10_digits() == "3"
                );
                if is_f32 && len_is_three {
                    return Ok(ParamType::Vec3);
                }
            }
        }
        Err(syn::Error::new(
            ty.span(),
            format!(
                "FeatureSchema does not yet support field type `{}`. \
                 Supported: f32, [f32; 3], bool, String, BoolOp.",
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

