//! `#[derive(EngineClass)]`: a console class declared as a Rust struct, one typed field for each of its console fields.
//!
//! ```text
//! #[derive(EngineClass)]
//! #[class(name = "RopeData", parent = "SimDataBlock", preload = RopeData::check)]
//! struct RopeData {
//!     /// How many pieces the rope is cut into.
//!     #[field(range = 2..=64)]
//!     segments: i32,
//!     length: f32,
//!     material: Filename,
//! }
//! ```
//!
//! The expansion is an `impl ScriptClass`: the class's name and parent, one `FieldDecl` constant per field (the console
//! field's name, its type from the Rust type, doc comment, default and range), the reader that builds the struct from an
//! object's fields, and the `preload` hook. What the engine knows by type (which console field type a Rust type is, how
//! its stored value reads back) stays in `ClassField`, so a field of a type it lacks does not compile.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use crate::attrs::{closed_range, literal_text, named_fields, value_checks};
use crate::native::is_script_name;
use crate::{camel_case, doc_text};

/// The expansion of `#[derive(EngineClass)]`, with `engine` the engine crate's path
/// ([`find_engine`](crate::find_engine)).
///
/// The struct takes `#[class(name = "RopeData", parent = "SimDataBlock", preload = path)]`: `name` is the console class
/// (the struct's name when left out), `parent` the class it goes below (`SimObject` when left out), `preload` a function
/// `fn(&Self, &mut Vm, ObjId) -> Result<(), String>` that runs for each datablock of the class once it is registered
/// (only datablocks are preloaded), with its fields read into the struct (an `Err` prints the preload failure and
/// deletes the datablock), and
/// `crate = path` the engine crate. Each field takes `#[field(...)]`: `name = ".."` (the console field's name: the Rust
/// name in camelCase when left out), `doc = ".."` (the doc comment when left out), `range = low..=high` (writes are
/// clamped to it), and `default = ..` (a literal: what a fresh object reads).
pub fn derive_class(input: TokenStream, engine: &syn::Path) -> TokenStream {
    expand(input, engine).unwrap_or_else(syn::Error::into_compile_error)
}

fn expand(input: TokenStream, engine: &syn::Path) -> syn::Result<TokenStream> {
    let input: syn::DeriveInput = syn::parse2(input)?;
    let fields = named_fields(&input, "a console class")?;
    let mut name: Option<LitStr> = None;
    let mut parent: Option<LitStr> = None;
    let mut preload: Option<syn::Path> = None;
    let mut krate: Option<syn::Path> = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("class")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(class_name(meta.value()?.parse()?)?);
            } else if meta.path.is_ident("parent") {
                parent = Some(class_name(meta.value()?.parse()?)?);
            } else if meta.path.is_ident("preload") {
                preload = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("crate") {
                krate = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error(
                    "expected `name = \"...\"`, `parent = \"...\"`, `preload = path` or `crate = path`",
                ));
            }
            Ok(())
        })?;
    }
    let krate = krate.unwrap_or_else(|| engine.clone());
    let ident = &input.ident;
    let name = name.map_or_else(|| ident.to_string(), |n| n.value());
    let parent = parent.map_or_else(|| "SimObject".to_string(), |p| p.value());
    let trait_path = quote! { #krate::class::ClassField };

    let mut decls = Vec::new();
    let mut reads = Vec::new();
    let mut checks = Vec::new();
    let mut taken: Vec<(String, &syn::Ident)> = Vec::new();
    for (index, field) in fields.named.iter().enumerate() {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let options = field_options(field)?;
        let console = options
            .name
            .as_ref()
            .map_or_else(|| camel_case(&field_ident.to_string()), LitStr::value);
        if let Some((_, first)) = taken.iter().find(|(n, _)| n.eq_ignore_ascii_case(&console)) {
            return Err(syn::Error::new_spanned(
                field_ident,
                format!(
                    "`{console}` is also the console field of `{first}` (names compare without case): give one a `name = \"...\"`"
                ),
            ));
        }
        taken.push((console.clone(), field_ident));
        let docs = options
            .doc
            .as_ref()
            .map_or_else(|| doc_text(&field.attrs), LitStr::value);
        let ty = &field.ty;
        let default = match &options.default {
            Some(default) => {
                let text = literal_text(default)?;
                quote! { ::core::option::Option::Some(#text) }
            }
            None => quote! { ::core::option::Option::None },
        };
        let (min, max) = match &options.range {
            Some(range) => {
                let (low, high) = closed_range(range)?;
                (
                    quote! { ::core::option::Option::Some((#low) as f64) },
                    quote! { ::core::option::Option::Some((#high) as f64) },
                )
            }
            None => (
                quote! { ::core::option::Option::None },
                quote! { ::core::option::Option::None },
            ),
        };
        checks.push(value_checks(
            ty,
            &trait_path,
            options.default.as_ref(),
            options.range.as_ref(),
        )?);
        let field_name = field_ident.to_string();
        decls.push(quote! {
            #krate::FieldDecl {
                name: #console,
                field: #field_name,
                ty: <#ty as #trait_path>::TYPE,
                doc: #docs,
                default: #default,
                min: #min,
                max: #max,
            }
        });
        reads.push(quote! {
            #field_ident: #krate::class::read_field::<#ty>(
                &<Self as #krate::ScriptClass>::FIELDS[#index],
                &mut *get,
            )?
        });
    }
    let hook = preload.map(|path| {
        quote! {
            const HAS_PRELOAD: bool = true;
            fn preload(
                &self,
                vm: &mut #krate::class::Vm,
                id: #krate::class::ObjId,
            ) -> ::core::result::Result<(), ::std::string::String> {
                #path(self, vm, id)
            }
        }
    });
    Ok(quote! {
        impl #krate::ScriptClass for #ident {
            const NAME: &'static str = #name;
            const PARENT: &'static str = #parent;
            const FIELDS: &'static [#krate::FieldDecl] = &[#(#decls),*];
            #hook
            fn from_values(
                get: &mut dyn ::core::ops::FnMut(
                    &#krate::FieldDecl,
                ) -> ::core::option::Option<#krate::class::FieldValue>,
            ) -> ::core::result::Result<Self, &'static str> {
                ::core::result::Result::Ok(Self { #(#reads),* })
            }
        }
        #(#checks)*
    })
}

fn class_name(name: LitStr) -> syn::Result<LitStr> {
    if is_script_name(&name.value()) {
        Ok(name)
    } else {
        Err(syn::Error::new_spanned(
            &name,
            "a class name is ASCII letters, digits and '_', not starting with a digit",
        ))
    }
}

/// What a field's `#[field(...)]` says.
#[derive(Default)]
struct FieldOptions {
    default: Option<syn::Expr>,
    range: Option<syn::Expr>,
    name: Option<LitStr>,
    doc: Option<LitStr>,
}

fn field_options(field: &syn::Field) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("field")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                options.default = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("range") {
                let range: syn::Expr = meta.value()?.parse()?;
                closed_range(&range)?;
                options.range = Some(range);
            } else if meta.path.is_ident("name") {
                let name: LitStr = meta.value()?.parse()?;
                if !is_script_name(&name.value()) {
                    return Err(syn::Error::new_spanned(
                        &name,
                        "a field's name is ASCII letters, digits and '_', not starting with a digit",
                    ));
                }
                options.name = Some(name);
            } else if meta.path.is_ident("doc") {
                options.doc = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error(
                    "expected `default = ..`, `range = low..=high`, `name = \"...\"` or `doc = \"...\"`",
                ));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

#[cfg(test)]
#[path = "class_tests.rs"]
mod tests;
