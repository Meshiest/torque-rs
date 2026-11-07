//! The code generation behind the `torque-macros` attributes.
//!
//! A proc-macro crate can export nothing but macros, and its functions run only inside the compiler, so the work lives
//! here as plain functions from tokens to tokens: a test feeds one a [`TokenStream`] and compares what comes back, with
//! no compiler in the loop. `torque-macros` holds the thin entry points that call these.

use proc_macro2::TokenStream;
use quote::ToTokens;

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

#[cfg(test)]
mod tests;
