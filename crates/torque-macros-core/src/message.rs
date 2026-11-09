//! `#[derive(NetMessage)]`: the request or reply of a plugin's RPC declared as a struct, one typed field for each value.
//!
//! ```text
//! #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
//! struct SpawnProp {
//!     #[net(max_len = 64)]
//!     data: String,
//!     #[net(finite)]
//!     at: Xf,
//!     #[net(bits = 8)]
//!     predict_id: u8,
//! }
//! ```
//!
//! The expansion is the message's layout (the same `impl Layout` a ghost record gets: one codec call per field, in the struct's
//! order) and an `impl NetMessage`: the layout as text (what the manifest's digest covers), the ghost references the message
//! names, the finite check, and the conversions a script call needs. A message comes from a peer that chooses its bytes, so
//! it is stricter than a record: a `String` or `Vec` must say its `max_len`, a float field must say `finite`, and a field of
//! a type the wire has no form for does not compile.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use crate::attrs::named_fields;
use crate::native::is_script_name;

/// The expansion of `#[derive(NetMessage)]`, with `engine` the engine crate's path ([`find_engine`](crate::find_engine)).
///
/// The struct may take `#[net(crate = path)]`. Each field takes `#[net(...)]`: `bits = N` (an integer's width), `max_len = N`
/// (a string's bytes or a list's elements: required for those types) and `finite` (a float field's decoder refuses a NaN
/// or an infinity: required for float types).
pub fn derive_net_message(input: TokenStream, engine: &syn::Path) -> TokenStream {
    expand(input, engine).unwrap_or_else(syn::Error::into_compile_error)
}

#[derive(Default)]
struct FieldOptions {
    bits: Option<syn::LitInt>,
    max_len: Option<syn::LitInt>,
    finite: bool,
}

fn field_options(field: &syn::Field) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("net")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("bits") {
                let bits: syn::LitInt = meta.value()?.parse()?;
                if !matches!(bits.base10_parse::<u8>(), Ok(1..=32)) {
                    return Err(syn::Error::new_spanned(&bits, "`bits` is 1 to 32"));
                }
                options.bits = Some(bits);
            } else if meta.path.is_ident("max_len") {
                let len: syn::LitInt = meta.value()?.parse()?;
                if !matches!(len.base10_parse::<usize>(), Ok(1..)) {
                    return Err(syn::Error::new_spanned(&len, "`max_len` is at least 1"));
                }
                options.max_len = Some(len);
            } else if meta.path.is_ident("finite") {
                options.finite = true;
            } else if meta.path.is_ident("skip")
                || meta.path.is_ident("mask")
                || meta.path.is_ident("name")
            {
                return Err(meta.error(
                    "a message sends every field as it is: it takes no `skip`, `mask` or `name`",
                ));
            } else {
                return Err(meta.error("expected `bits = N`, `max_len = N` or `finite`"));
            }
            Ok(())
        })?;
    }
    Ok(options)
}

fn expand(input: TokenStream, engine: &syn::Path) -> syn::Result<TokenStream> {
    let input: syn::DeriveInput = syn::parse2(input)?;
    let fields = named_fields(&input, "a message")?;
    let mut krate: Option<syn::Path> = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("net")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                krate = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("expected `crate = path`"))
            }
        })?;
    }
    let krate = krate.unwrap_or_else(|| engine.clone());
    let g = quote! { #krate::__ghosts };
    let ident = &input.ident;
    let field_trait = quote! { #g::plugin::NetField };

    let mut layouts = Vec::new();
    let mut names = Vec::new();
    let mut refs = Vec::new();
    let mut maps = Vec::new();
    let mut checks = Vec::new();
    let mut valid = Vec::new();
    let mut reads = Vec::new();
    let mut values = Vec::new();
    let mut schema = Vec::new();
    for field in &fields.named {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let name = field_ident.to_string();
        if !is_script_name(&name) {
            return Err(syn::Error::new_spanned(
                field_ident,
                "a field's name is ASCII letters, digits and '_', not starting with a digit",
            ));
        }
        let options = field_options(field)?;
        let ty = &field.ty;
        let bits = options
            .bits
            .as_ref()
            .map_or(Ok(0u8), syn::LitInt::base10_parse::<u8>)?;
        let max_len = options
            .max_len
            .as_ref()
            .map_or(Ok(0usize), syn::LitInt::base10_parse::<usize>)?;
        let spec = quote! { #g::plugin::FieldSpec { bits: #bits, max_len: #max_len } };
        if bits > 0 {
            let message = format!("`bits = {bits}` is more than `{name}`'s type takes");
            checks.push(
                quote! { ::core::assert!(#bits <= <#ty as #field_trait>::MAX_BITS, #message); },
            );
        }
        if max_len > 0 {
            let message = format!("`max_len = {max_len}` is more than `{name}`'s type takes");
            checks.push(
                quote! { ::core::assert!(#max_len <= <#ty as #field_trait>::MAX_LEN, #message); },
            );
        } else {
            let message = format!(
                "`{name}` needs a `max_len`: a message has no default cap on a string or a list"
            );
            checks.push(quote! { ::core::assert!(!<#ty as #field_trait>::NEEDS_CAP, #message); });
        }
        if options.finite {
            let message = format!("`{name}` holds no floats, so `finite` does nothing");
            checks.push(quote! { ::core::assert!(<#ty as #field_trait>::HAS_FLOATS, #message); });
            valid.push(quote! {
                if !#field_trait::finite(&self.#field_ident) {
                    return ::core::result::Result::Err(#g::plugin::message::non_finite());
                }
            });
        } else {
            let message = format!(
                "`{name}` holds floats: mark it `#[net(finite)]` so a NaN or an infinity from a peer is refused"
            );
            checks.push(quote! { ::core::assert!(!<#ty as #field_trait>::HAS_FLOATS, #message); });
        }
        let ty_text = quote!(#ty).to_string().replace(' ', "");
        if mentions_object_id(ty) {
            return Err(syn::Error::new_spanned(
                ty,
                "a raw object id is not a message field: the id means nothing to the peer and a peer could make one up; name an object with `GhostRef`, which the server checks against the ghosts it sent the caller",
            ));
        }
        schema.push(format!(
            "{name}:{ty_text}:{bits}:{max_len}:{}",
            u8::from(options.finite)
        ));
        let lit = LitStr::new(&name, field_ident.span());
        layouts.push(quote! {
            #field_trait::layout(&mut self.#field_ident, c, #lit, #spec)?;
        });
        names.push(quote! { #lit });
        refs.push(quote! { #field_trait::ghost_refs(&self.#field_ident, out); });
        maps.push(quote! { ok &= #field_trait::map_ghosts(&mut self.#field_ident, map); });
        reads.push(quote! {
            #field_ident: <#ty as #field_trait>::from_view(&view, #lit, 0, #spec)
        });
        values.push(quote! { #field_trait::values(&self.#field_ident, &mut out); });
    }
    let schema = schema.join(";");
    Ok(quote! {
        impl #g::codec::Layout for #ident {
            fn layout<C: #g::codec::Codec>(&mut self, c: &mut C) -> #g::codec::R {
                let _ = &c;
                #(#layouts)*
                ::core::result::Result::Ok(())
            }
        }
        impl #g::plugin::NetMessage for #ident {
            const SCHEMA: &'static str = #schema;

            fn field_names() -> &'static [&'static str] {
                &[#(#names),*]
            }

            fn ghost_refs(&self, out: &mut ::std::vec::Vec<u32>) {
                let _ = &out;
                #(#refs)*
            }

            fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> ::core::option::Option<u32>) -> bool {
                let mut ok = true;
                #(#maps)*
                ok
            }

            fn validate(&self) -> ::core::result::Result<(), #g::plugin::message::NetError> {
                #(#valid)*
                ::core::result::Result::Ok(())
            }

            fn from_script(args: &[::std::vec::Vec<u8>]) -> Self {
                let view = #g::plugin::ScriptArgs::new(
                    <Self as #g::plugin::NetMessage>::field_names(),
                    args,
                );
                let _ = &view;
                Self { #(#reads),* }
            }

            fn script_values(&self) -> ::std::vec::Vec<#g::Value> {
                let mut out = ::std::vec::Vec::new();
                #(#values)*
                out
            }
        }
        const _: () = { #(#checks)* };
    })
}

/// Whether the type is, or holds as an argument, a type named `ObjId` (the script engine's raw object id, which is an alias of
/// `u32`, so only its name tells it from a number).
fn mentions_object_id(ty: &syn::Type) -> bool {
    use syn::visit::Visit;
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
            self.0 |= segment.ident == "ObjId";
            syn::visit::visit_path_segment(self, segment);
        }
    }
    let mut finder = Finder(false);
    finder.visit_type(ty);
    finder.0
}
