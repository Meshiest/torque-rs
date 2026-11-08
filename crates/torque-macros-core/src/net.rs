//! `#[derive(GhostRecord)]` and `#[derive(DatablockCodec)]`: a plugin's ghost or datablock record declared as a struct, one
//! typed field for each value it sends.
//!
//! ```text
//! #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
//! struct RopeGhost {
//!     #[net(bits = 6)]
//!     segments: u32,
//!     length: f32,
//!     #[net(max_len = 32)]
//!     label: String,
//!     #[net(mask = 0x2)]
//!     color: Option<[f32; 3]>,
//! }
//! ```
//!
//! The expansion is the record's layout (an `impl Layout`: one codec call per field, in the struct's order, with its width
//! and cap), an `impl NetRecord` (the merge of an update, the datablock and transform the client's world asks for, the
//! script fields) and the packer (`impl GhostRecord` or `impl DatablockCodec`), which reads each field from the host's view
//! by its console field name. What the engine knows by type (how a type travels, the widths and caps it takes) stays in
//! `NetField`, so a field of a type it lacks, or an option the type does not take, does not compile.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use crate::attrs::named_fields;
use crate::camel_case;
use crate::native::is_script_name;

/// The record's kind: what the derive's packer is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ghost,
    Datablock,
}

impl Kind {
    fn what(self) -> &'static str {
        match self {
            Kind::Ghost => "a ghost record",
            Kind::Datablock => "a datablock record",
        }
    }
}

/// The expansion of `#[derive(GhostRecord)]`, with `engine` the engine crate's path ([`find_engine`](crate::find_engine)).
///
/// The struct may take `#[net(crate = path)]` (the engine crate otherwise). Each field takes `#[net(...)]`: `bits = N` (an
/// integer's width, 1 to the type's), `max_len = N` (a string's bytes or a list's elements), `mask = bits` (an `Option`
/// sent only when one of the dirty bits is set: every bit is set for a new ghost), `name = ".."` (the console field the
/// packer reads: an `#[field(name = "..")]`'s, else the Rust name in camelCase) and `skip` (not sent: the decoder leaves the default).
pub fn derive_ghost_record(input: TokenStream, engine: &syn::Path) -> TokenStream {
    expand(input, engine, Kind::Ghost).unwrap_or_else(syn::Error::into_compile_error)
}

/// The expansion of `#[derive(DatablockCodec)]`: as [`derive_ghost_record`], with the datablock's packer and no `mask` (a
/// datablock goes out whole).
pub fn derive_datablock_codec(input: TokenStream, engine: &syn::Path) -> TokenStream {
    expand(input, engine, Kind::Datablock).unwrap_or_else(syn::Error::into_compile_error)
}

/// What a field's `#[net(...)]` says.
#[derive(Default)]
struct FieldOptions {
    bits: Option<syn::LitInt>,
    max_len: Option<syn::LitInt>,
    mask: Option<syn::Expr>,
    name: Option<LitStr>,
    skip: bool,
}

fn field_options(field: &syn::Field, kind: Kind) -> syn::Result<FieldOptions> {
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
            } else if meta.path.is_ident("mask") {
                if kind == Kind::Datablock {
                    return Err(meta.error("a datablock goes out whole: its fields take no `mask`"));
                }
                options.mask = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("name") {
                let name: LitStr = meta.value()?.parse()?;
                if !is_script_name(&name.value()) {
                    return Err(syn::Error::new_spanned(
                        &name,
                        "a field's name is ASCII letters, digits and '_', not starting with a digit",
                    ));
                }
                options.name = Some(name);
            } else if meta.path.is_ident("skip") {
                options.skip = true;
            } else {
                return Err(meta.error(
                    "expected `bits = N`, `max_len = N`, `mask = bits`, `name = \"...\"` or `skip`",
                ));
            }
            Ok(())
        })?;
    }
    if options.name.is_none() {
        options.name = console_name(field)?;
    }
    if options.skip
        && (options.bits.is_some() || options.max_len.is_some() || options.mask.is_some())
    {
        return Err(syn::Error::new_spanned(
            field,
            "a `skip` field is not sent: it takes no `bits`, `max_len` or `mask`",
        ));
    }
    Ok(options)
}

/// The console field name an `#[derive(EngineClass)]` on the same struct gives the field (`#[field(name = "...")]`), so
/// the packer reads the field the class has; its other options are that derive's.
fn console_name(field: &syn::Field) -> syn::Result<Option<LitStr>> {
    let mut name = None;
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("field")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = Some(meta.value()?.parse()?);
            } else if let Ok(value) = meta.value() {
                value.parse::<syn::Expr>()?;
            }
            Ok(())
        })?;
    }
    Ok(name)
}

/// The record crate's path, reached through the engine crate (`#[doc(hidden)]` there).
fn ghosts_path(krate: &syn::Path) -> TokenStream {
    quote! { #krate::__ghosts }
}

fn expand(input: TokenStream, engine: &syn::Path, kind: Kind) -> syn::Result<TokenStream> {
    let input: syn::DeriveInput = syn::parse2(input)?;
    let fields = named_fields(&input, kind.what())?;
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
    let g = ghosts_path(&krate);
    let ident = &input.ident;
    let field_trait = quote! { #g::plugin::NetField };

    let mut layouts = Vec::new();
    let mut merges = Vec::new();
    let mut datablocks = Vec::new();
    let mut transforms = Vec::new();
    let mut values = Vec::new();
    let mut packs = Vec::new();
    let mut pendings = Vec::new();
    let mut checks = Vec::new();
    let mut taken: Vec<(String, &syn::Ident)> = Vec::new();
    for field in &fields.named {
        let Some(field_ident) = &field.ident else {
            continue;
        };
        let options = field_options(field, kind)?;
        let ty = &field.ty;
        if options.skip {
            packs.push(quote! { #field_ident: ::core::default::Default::default() });
            continue;
        }
        let name = options
            .name
            .as_ref()
            .map_or_else(|| camel_case(&field_ident.to_string()), LitStr::value);
        if let Some((_, first)) = taken.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            return Err(syn::Error::new_spanned(
                field_ident,
                format!(
                    "`{name}` is also the field name of `{first}` (names compare without case): give one a `name = \"...\"`"
                ),
            ));
        }
        taken.push((name.clone(), field_ident));
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
            let message = format!("`bits = {bits}` is more than the field's type takes");
            checks.push(
                quote! { ::core::assert!(#bits <= <#ty as #field_trait>::MAX_BITS, #message); },
            );
        }
        if max_len > 0 {
            let message = format!("`max_len = {max_len}` is more than the field's type takes");
            checks.push(
                quote! { ::core::assert!(#max_len <= <#ty as #field_trait>::MAX_LEN, #message); },
            );
        }
        layouts.push(quote! {
            #field_trait::layout(&mut self.#field_ident, c, #name, #spec)?;
        });
        merges
            .push(quote! { #field_trait::merge_from(&mut self.#field_ident, &sent.#field_ident); });
        datablocks.push(quote! { .or_else(|| #field_trait::datablock(&self.#field_ident)) });
        transforms.push(quote! { .or_else(|| #field_trait::transform(&self.#field_ident)) });
        values.push(quote! { #g::plugin::push_values(&mut out, #name, &self.#field_ident); });
        let read = quote! { <#ty as #field_trait>::from_view(view, #name, 0, #spec) };
        match &options.mask {
            Some(mask) => {
                let message = format!(
                    "`{field_ident}` has a `mask`, so it must be an `Option` (none while the mask is clear)"
                );
                checks.push(quote! { ::core::assert!(<#ty as #field_trait>::OPTIONAL, #message); });
                packs.push(quote! {
                    #field_ident: if mask & ((#mask) as u32) != 0 {
                        #read
                    } else {
                        ::core::default::Default::default()
                    }
                });
                pendings.push(quote! {
                    if #field_trait::pending(&record.#field_ident) {
                        kept |= (#mask) as u32;
                    }
                });
            }
            None => packs.push(quote! { #field_ident: #read }),
        }
    }
    let packer = match kind {
        Kind::Ghost => quote! {
            impl #g::plugin::GhostRecord for #ident {
                fn pack(
                    view: &dyn #g::ObjectView,
                    mask: u32,
                    ctx: &#g::PackCtx,
                ) -> (Self, u32) {
                    let _ = (mask, ctx);
                    let record = Self { #(#packs),* };
                    let mut kept = 0u32;
                    #(#pendings)*
                    (record, kept)
                }
            }
        },
        Kind::Datablock => quote! {
            impl #g::plugin::DatablockCodec for #ident {
                fn pack(view: &dyn #g::ObjectView, widths: #g::codec::NetWidths) -> Self {
                    let _ = widths;
                    Self { #(#packs),* }
                }
            }
        },
    };
    let checks = if checks.is_empty() {
        quote! {}
    } else {
        quote! { const _: () = { #(#checks)* }; }
    };
    Ok(quote! {
        impl #g::codec::Layout for #ident {
            fn layout<C: #g::codec::Codec>(&mut self, c: &mut C) -> #g::codec::R {
                let _ = &c;
                #(#layouts)*
                ::core::result::Result::Ok(())
            }
        }
        impl #g::plugin::NetRecord for #ident {
            fn merge(&mut self, sent: &Self) {
                let _ = sent;
                #(#merges)*
            }
            fn datablock(&self) -> ::core::option::Option<u32> {
                ::core::option::Option::None #(#datablocks)*
            }
            fn transform(&self) -> ::core::option::Option<#g::codec::Xf> {
                ::core::option::Option::None #(#transforms)*
            }
            fn script_fields(&self) -> ::std::vec::Vec<(::std::string::String, u32, #g::Value)> {
                let mut out = ::std::vec::Vec::new();
                #(#values)*
                out
            }
        }
        #packer
        #checks
    })
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;
