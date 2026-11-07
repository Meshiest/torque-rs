//! The code generation behind the `torque-macros` attributes.
//!
//! A proc-macro crate can export nothing but macros, and its functions run only inside the compiler, so the work lives
//! here as plain functions from tokens to tokens: a test feeds one a [`TokenStream`] and compares what comes back, with
//! no compiler in the loop. `torque-macros` holds the thin entry points that call these.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};

/// The expansion of `#[passthrough]`: the item exactly as written.
///
/// The attribute takes no arguments, and the item must be Rust that parses as one. A refusal comes back as the
/// `compile_error!` the compiler shows at the offending tokens.
pub fn passthrough(args: TokenStream, item: TokenStream) -> TokenStream {
    expand_passthrough(args, item).unwrap_or_else(syn::Error::into_compile_error)
}

fn expand_passthrough(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !args.is_empty() {
        return Err(syn::Error::new_spanned(
            args,
            "this attribute takes no arguments",
        ));
    }
    let item: syn::Item = syn::parse2(item)?;
    Ok(item.into_token_stream())
}

/// The expansion of `#[derive(PluginId)]`: an `impl PluginType` for the type, with its stable id.
///
/// The id is `#[plugin(id = "...")]` when given (checked: ASCII letters, digits, `_`, `-`, `.` and `:`), and
/// otherwise the deriving package's name `crate_name` and the type's name as a path ([`default_id`]):
/// `torque_audio::AudioPlugin`. The impl names the engine crate by `engine` ([`find_engine`]) unless
/// `#[plugin(crate = path)]` gives another path. Generic types are refused: every instance would share one id.
pub fn derive_plugin_id(input: TokenStream, crate_name: &str, engine: &syn::Path) -> TokenStream {
    expand_plugin_id(input, crate_name, engine).unwrap_or_else(syn::Error::into_compile_error)
}

/// The engine crate's path in the crate being compiled, from its manifest: see [`engine_path`].
pub fn find_engine() -> syn::Path {
    engine_path(
        proc_macro_crate::crate_name("torque-engine").ok(),
        proc_macro_crate::crate_name("torque").ok(),
    )
}

/// The engine crate's path, from where its manifest has `torque-engine` and `torque`: `torque-engine` by its name (as
/// renamed there) when the crate depends on it, else the umbrella crate's `engine` module, else `::torque_engine`
/// (the engine crate itself, whose tests and examples name it so).
pub fn engine_path(
    engine: Option<proc_macro_crate::FoundCrate>,
    umbrella: Option<proc_macro_crate::FoundCrate>,
) -> syn::Path {
    use proc_macro_crate::FoundCrate;
    let ident = |name: &str| syn::Ident::new(name, proc_macro2::Span::call_site());
    match (engine, umbrella) {
        (Some(FoundCrate::Name(name)), _) => {
            let name = ident(&name);
            syn::parse_quote!(::#name)
        }
        (None, Some(FoundCrate::Name(name))) => {
            let name = ident(&name);
            syn::parse_quote!(::#name::engine)
        }
        (None, Some(FoundCrate::Itself)) => syn::parse_quote!(::torque::engine),
        _ => syn::parse_quote!(::torque_engine),
    }
}

fn expand_plugin_id(
    input: TokenStream,
    crate_name: &str,
    engine: &syn::Path,
) -> syn::Result<TokenStream> {
    let input: syn::DeriveInput = syn::parse2(input)?;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "a plugin id is one per type: implement `PluginType` by hand for a generic plugin",
        ));
    }
    let mut id: Option<syn::LitStr> = None;
    let mut krate: Option<syn::Path> = None;
    for attr in input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("plugin"))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("id") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                if !is_valid_id(&lit.value()) {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "a plugin id is ASCII letters, digits, '_', '-', '.' and ':', and not empty",
                    ));
                }
                id = Some(lit);
                Ok(())
            } else if meta.path.is_ident("crate") {
                krate = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("expected `id = \"...\"` or `crate = path`"))
            }
        })?;
    }
    let ident = &input.ident;
    let id = match id {
        Some(lit) => lit.value(),
        None if crate_name.is_empty() => {
            return Err(syn::Error::new_spanned(
                ident,
                "no package name to make the id from (CARGO_PKG_NAME): give one with `#[plugin(id = \"...\")]`",
            ));
        }
        None => default_id(crate_name, &ident.to_string()),
    };
    let krate = krate.unwrap_or_else(|| engine.clone());
    Ok(quote! {
        impl #krate::PluginType for #ident {
            const ID: #krate::PluginId = #krate::PluginId::new(#id);
        }
    })
}

/// The id of type `ty` in the package `package`: the two as a Rust path, `torque_audio::AudioPlugin`. The package's
/// `-` become `_`, as in its crate name, and anything else outside an id's characters becomes `_`.
pub fn default_id(package: &str, ty: &str) -> String {
    let clean = |c: char| match c {
        '-' => '_',
        c if c.is_ascii_alphanumeric() || c == '_' => c,
        _ => '_',
    };
    let mut id: String = package.chars().map(clean).collect();
    id.push_str("::");
    id.extend(ty.chars().map(clean));
    id
}

/// The engine's rule for an id (`torque_engine::PluginId::is_valid`), checked here so a bad id shows at its tokens.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

#[cfg(test)]
mod tests;
