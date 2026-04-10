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
//! ## Minimal scope for this slice
//!
//! K4.5 step 3 sets the proc-macro infrastructure down and proves
//! the schema-plus-parse pipeline on a handful of primitive field
//! types. Later steps will grow the derive:
//!
//! * **Supported field types:** `f32`, `[f32; 3]`, `bool`, `String`.
//!   `Length` / `Angle` / `Query` / `Option<T>` come later — they
//!   need runtime types in the kernel that don't exist yet.
//! * **Attributes:** only the struct-level `#[feature(type = "…")]`
//!   is honoured. The field-level `#[param(name, bounds, default,
//!   visible_when, filter, ui_hint)]` attribute from §4.6 is not
//!   recognised yet — every field is implicitly required, its
//!   display name defaults to its Rust identifier and its key in
//!   the definition JSON is the snake_case Rust name.
//! * **No `impl FeatureKind`.** The derive generates schema /
//!   parse helpers only. Callers still write `FeatureKind` by
//!   hand, just shorter — the hand-written `execute` function
//!   starts with `let def = MyDef::parse(def)?;` instead of
//!   reaching into the JSON directly.
//!
//! The minimum is enough to unblock K4.5 steps 4+ without dragging
//! the full Onshape-style schema language into this slice.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned, ToTokens};
use syn::{
    parse_macro_input, spanned::Spanned, Data, DeriveInput, Expr, ExprLit,
    Fields, Lit, Meta, MetaNameValue, Type, TypeArray, TypePath,
};

/// Derive macro for `FeatureSchema`. See the crate docs for the
/// full input-shape and output contract.
#[proc_macro_derive(FeatureSchema, attributes(schema))]
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
    Vec3,
    Bool,
    String,
}

impl ParamType {
    fn json_type_name(&self) -> &'static str {
        match self {
            ParamType::F32 => "f32",
            ParamType::Vec3 => "vec3",
            ParamType::Bool => "bool",
            ParamType::String => "string",
        }
    }
}

struct ParamInfo {
    ident: syn::Ident,
    key: String,
    display_name: String,
    ty: ParamType,
}

impl ParamInfo {
    fn from_field(field: &syn::Field) -> syn::Result<Self> {
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(field.span(), "unnamed field"))?;
        let key = ident.to_string();
        let display_name = prettify_identifier(&key);
        let ty = ParamType::from_type(&field.ty)?;
        Ok(ParamInfo {
            ident,
            key,
            display_name,
            ty,
        })
    }

    fn emit_schema_entry(&self) -> TokenStream2 {
        let key = &self.key;
        let name = &self.display_name;
        let type_name = self.ty.json_type_name();
        quote! {
            ::serde_json::json!({
                "key": #key,
                "name": #name,
                "type": #type_name,
                "required": true,
            })
        }
    }

    /// Emit a `let <ident> = ...?;` binding that pulls the
    /// parameter out of the definition JSON and validates its
    /// type. The generated code borrows names from the kernel
    /// crate's `crate::features::FeatureError` so the derive can
    /// stay type-agnostic here.
    fn emit_parse_binding(&self) -> TokenStream2 {
        let ident = &self.ident;
        let key = &self.key;
        let feature_literal = quote! { Self::TYPE_NAME };
        match self.ty {
            ParamType::F32 => quote! {
                let #ident: f32 = {
                    let slot = def.get(#key).ok_or(
                        crate::features::FeatureError::MissingParameter {
                            feature: #feature_literal,
                            name: #key,
                        },
                    )?;
                    slot.as_f64().map(|v| v as f32).ok_or_else(|| {
                        crate::features::FeatureError::InvalidParameter {
                            feature: #feature_literal,
                            name: #key,
                            message: "expected a number".to_string(),
                        }
                    })?
                };
            },
            ParamType::Vec3 => quote! {
                let #ident: [f32; 3] = {
                    let slot = def.get(#key).ok_or(
                        crate::features::FeatureError::MissingParameter {
                            feature: #feature_literal,
                            name: #key,
                        },
                    )?;
                    let arr = slot.as_array().ok_or_else(|| {
                        crate::features::FeatureError::InvalidParameter {
                            feature: #feature_literal,
                            name: #key,
                            message: "expected a 3-element array".to_string(),
                        }
                    })?;
                    if arr.len() != 3 {
                        return ::core::result::Result::Err(
                            crate::features::FeatureError::InvalidParameter {
                                feature: #feature_literal,
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
                                feature: #feature_literal,
                                name: #key,
                                message: ::std::format!(
                                    "element {} is not a number",
                                    i
                                ),
                            },
                        )?;
                    }
                    out
                };
            },
            ParamType::Bool => quote! {
                let #ident: bool = {
                    let slot = def.get(#key).ok_or(
                        crate::features::FeatureError::MissingParameter {
                            feature: #feature_literal,
                            name: #key,
                        },
                    )?;
                    slot.as_bool().ok_or_else(|| {
                        crate::features::FeatureError::InvalidParameter {
                            feature: #feature_literal,
                            name: #key,
                            message: "expected a boolean".to_string(),
                        }
                    })?
                };
            },
            ParamType::String => quote! {
                let #ident: ::std::string::String = {
                    let slot = def.get(#key).ok_or(
                        crate::features::FeatureError::MissingParameter {
                            feature: #feature_literal,
                            name: #key,
                        },
                    )?;
                    slot.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        crate::features::FeatureError::InvalidParameter {
                            feature: #feature_literal,
                            name: #key,
                            message: "expected a string".to_string(),
                        }
                    })?
                };
            },
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
                 Supported: f32, [f32; 3], bool, String.",
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

// The `quote_spanned` / `MetaNameValue` / `Meta` imports are
// placeholders for the attribute features that K4.5 step 4+ will
// add (defaults, bounds, visible_when). Keep the imports to
// document the planned API without tripping the unused-import lint.
#[allow(dead_code)]
fn _reserved_for_future_param_attribute(
    _m: Option<MetaNameValue>,
    _meta: Option<Meta>,
    _span: Option<TokenStream2>,
) {
    let _ = quote_spanned! { proc_macro2::Span::call_site() => };
}
