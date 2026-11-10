//! `#[derive(Prefs)]`: a struct of typed preferences kept in `$Pref::...` globals.
//!
//! ```text
//! #[derive(Prefs)]
//! #[prefs(prefix = "$Pref::Pad")]
//! struct PadPrefs {
//!     /// How far the stick must move before it counts.
//!     #[pref(default = 0.15, range = 0.0..=0.9)]
//!     dead_zone: f32,
//!     #[pref(default = true)]
//!     invert_y: bool,
//! }
//! ```
//!
//! The expansion is an `impl Prefs`: the prefix, one `PrefDecl` constant per field (the global's name, kind, doc
//! comment, default as the text a script holds, and range), and the reader that builds the struct from the globals. What
//! the engine knows by type (how text reads as an `f32`, how a value is clamped) stays in `PrefType`, so a field of a
//! type it lacks does not compile.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use crate::attrs::{closed_range, literal_text, named_fields, pascal_case, value_checks};
use crate::doc_text;
use crate::native::is_script_name;

/// The expansion of `#[derive(Prefs)]`, with `engine` the engine crate's path ([`find_engine`](crate::find_engine)).
///
/// The struct takes `#[prefs(prefix = "$Pref::Pad")]` (required: `$`, then names joined by `::`; `crate = path` names
/// the engine crate otherwise). Each field takes `#[pref(default = ..)]` (required: a literal of the field's type, or
/// text), `range = low..=high` (the value is clamped to it when read), `name = "DeadZone"` (the global's last part:
/// the field's name in PascalCase when left out) and `doc = ".."` (the doc comment when left out). The global of a field
/// is the prefix, `::` and its name.
pub fn derive_prefs(input: TokenStream, engine: &syn::Path) -> TokenStream {
    expand(input, engine).unwrap_or_else(syn::Error::into_compile_error)
}

/// Whether `prefix` is a global's name up to a `::`: `$`, then names of letters, digits and `_` joined by `::`.
fn is_prefix(prefix: &str) -> bool {
    prefix
        .strip_prefix('$')
        .is_some_and(|rest| rest.split("::").all(is_script_name))
}

fn expand(input: TokenStream, engine: &syn::Path) -> syn::Result<TokenStream> {
    let input: syn::DeriveInput = syn::parse2(input)?;
    let fields = named_fields(&input, "a set of preferences")?;
    let mut prefix: Option<LitStr> = None;
    let mut krate: Option<syn::Path> = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("prefs")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("prefix") {
                let lit: LitStr = meta.value()?.parse()?;
                if !is_prefix(&lit.value()) {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "a prefix is `$`, then names of letters, digits and '_' joined by `::` (`$Pref::Pad`)",
                    ));
                }
                prefix = Some(lit);
            } else if meta.path.is_ident("crate") {
                krate = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error("expected `prefix = \"$Pref::...\"` or `crate = path`"));
            }
            Ok(())
        })?;
    }
    let Some(prefix) = prefix else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "give the prefix of the globals: `#[prefs(prefix = \"$Pref::Pad\")]`",
        ));
    };
    let krate = krate.unwrap_or_else(|| engine.clone());
    let ident = &input.ident;
    let trait_path = quote! { #krate::prefs::PrefType };

    let mut decls = Vec::new();
    let mut reads = Vec::new();
    let mut checks = Vec::new();
    let mut taken: Vec<(String, &syn::Ident)> = Vec::new();
    for (index, field) in fields.named.iter().enumerate() {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let options = field_options(field)?;
        let Some(default) = &options.default else {
            return Err(syn::Error::new_spanned(
                field_ident,
                "a pref needs a default: `#[pref(default = ..)]`",
            ));
        };
        let name = options
            .name
            .as_ref()
            .map_or_else(|| pascal_case(&field_ident.to_string()), LitStr::value);
        let global = format!("{}::{name}", prefix.value());
        if let Some((_, first)) = taken.iter().find(|(n, _)| n.eq_ignore_ascii_case(&global)) {
            return Err(syn::Error::new_spanned(
                field_ident,
                format!(
                    "`{global}` is also the global of `{first}` (names compare without case): give one a `name = \"...\"`"
                ),
            ));
        }
        taken.push((global.clone(), field_ident));
        let docs = options
            .doc
            .as_ref()
            .map_or_else(|| doc_text(&field.attrs), LitStr::value);
        let default_text = literal_text(default)?;
        let ty = &field.ty;
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
            Some(default),
            options.range.as_ref(),
        )?);
        let field_name = field_ident.to_string();
        decls.push(quote! {
            #krate::PrefDecl {
                name: #global,
                field: #field_name,
                kind: <#ty as #trait_path>::KIND,
                doc: #docs,
                default: #default_text,
                min: #min,
                max: #max,
            }
        });
        reads.push(quote! {
            #field_ident: #krate::prefs::read_pref::<#ty>(
                &<Self as #krate::Prefs>::PREFS[#index],
                &mut *read,
            )
        });
    }
    let prefix = prefix.value();
    Ok(quote! {
        impl #krate::Prefs for #ident {
            const PREFIX: &'static str = #prefix;
            const PREFS: &'static [#krate::PrefDecl] = &[#(#decls),*];
            fn read_with(
                read: &mut dyn ::core::ops::FnMut(
                    &#krate::PrefDecl,
                ) -> ::core::option::Option<::std::vec::Vec<u8>>,
            ) -> Self {
                Self { #(#reads),* }
            }
        }
        #(#checks)*
    })
}

/// What a field's `#[pref(...)]` says.
#[derive(Default)]
struct FieldOptions {
    default: Option<syn::Expr>,
    range: Option<syn::Expr>,
    name: Option<LitStr>,
    doc: Option<LitStr>,
}

fn field_options(field: &syn::Field) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("pref")) {
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
                        "a pref's name is ASCII letters, digits and '_', not starting with a digit",
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
#[path = "prefs_tests.rs"]
mod tests;
