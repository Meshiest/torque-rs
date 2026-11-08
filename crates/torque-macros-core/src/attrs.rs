//! What `#[derive(Prefs)]` and `#[derive(EngineClass)]` read the same way: literal values as the text a script holds,
//! `low..=high` ranges, and a struct's named fields.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

/// A literal value as the text a script holds it as: a number as written (without its suffix and `_`s), a truth value
/// as `1` or `0`, text as it is. A negative number is its literal after a `-`. Anything else is refused: the value is
/// text in a table, so it cannot be computed.
pub(crate) fn literal_text(expr: &syn::Expr) -> syn::Result<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit, .. }) => match lit {
            syn::Lit::Str(text) => Ok(text.value()),
            syn::Lit::Int(number) => Ok(number.base10_digits().to_string()),
            syn::Lit::Float(number) => Ok(number.base10_digits().to_string()),
            syn::Lit::Bool(truth) => Ok(if truth.value { "1" } else { "0" }.to_string()),
            other => Err(not_a_literal(other)),
        },
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr: inner,
            ..
        }) if matches!(
            &**inner,
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(_) | syn::Lit::Float(_),
                ..
            })
        ) =>
        {
            Ok(format!("-{}", literal_text(inner)?))
        }
        syn::Expr::Group(group) => literal_text(&group.expr),
        syn::Expr::Paren(paren) => literal_text(&paren.expr),
        other => Err(not_a_literal(other)),
    }
}

fn not_a_literal(tokens: impl quote::ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        tokens,
        "a default is a literal: a number, `true`/`false` or text (it is kept as text in a table, so it cannot be computed)",
    )
}

/// The two ends of `low..=high`; anything else is refused.
pub(crate) fn closed_range(expr: &syn::Expr) -> syn::Result<(&syn::Expr, &syn::Expr)> {
    if let syn::Expr::Range(range) = expr
        && matches!(range.limits, syn::RangeLimits::Closed(_))
        && let (Some(start), Some(end)) = (&range.start, &range.end)
    {
        return Ok((start, end));
    }
    Err(syn::Error::new(
        expr.span(),
        "a range is `low..=high`, both ends given",
    ))
}

/// The code that checks, when the crate is compiled, that a value's types fit its field's: the default is a
/// `literal` of the field's type, the range's ends are of the field's type, and the default is inside the range.
/// `ty` is the field's type, `engine` the path of the trait with the `Literal` associated type.
pub(crate) fn value_checks(
    ty: &syn::Type,
    trait_path: &TokenStream,
    default: Option<&syn::Expr>,
    range: Option<&syn::Expr>,
) -> syn::Result<TokenStream> {
    let mut checks = TokenStream::new();
    if let Some(default) = default {
        checks.extend(quote! {
            let _: <#ty as #trait_path>::Literal = #default;
        });
    }
    if let Some(range) = range {
        let (low, high) = closed_range(range)?;
        checks.extend(quote! {
            let _: ::core::ops::RangeInclusive<#ty> = #range;
        });
        if let Some(default) = default {
            checks.extend(quote! {
                ::core::assert!(
                    (#low) as f64 <= (#default) as f64 && (#default) as f64 <= (#high) as f64,
                    "the default is outside the range"
                );
            });
        }
    }
    Ok(quote! { const _: () = { #checks }; })
}

/// The named fields of the struct a derive is on; anything else is refused with `what` named.
pub(crate) fn named_fields<'a>(
    input: &'a syn::DeriveInput,
    what: &str,
) -> syn::Result<&'a syn::FieldsNamed> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            format!("{what} is one per type: a generic struct cannot be one"),
        ));
    }
    match &input.data {
        syn::Data::Struct(syn::DataStruct {
            fields: syn::Fields::Named(fields),
            ..
        }) => Ok(fields),
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            format!("{what} is a struct with named fields"),
        )),
    }
}

/// `dead_zone` as `DeadZone`: the words start with a capital.
pub fn pascal_case(name: &str) -> String {
    let camel = crate::camel_case(name);
    let mut chars = camel.chars();
    chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .chain(chars)
        .collect()
}
