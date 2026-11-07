use super::*;
use quote::quote;

/// Token streams compare by their text, which is spaced the same way for both sides.
fn same(a: &TokenStream, b: &TokenStream) {
    assert_eq!(a.to_string(), b.to_string());
}

#[test]
fn a_function_comes_back_unchanged() {
    let item = quote! {
        /// Adds.
        pub fn add(a: i32, b: i32) -> i32 { a + b }
    };
    same(&passthrough(TokenStream::new(), item.clone()), &item);
}

#[test]
fn other_items_come_back_unchanged() {
    for item in [
        quote! { struct Pad { dead_zone: f32 } },
        quote! { impl Pad { fn new() -> Self { Pad { dead_zone: 0.0 } } } },
        quote! { const LIMIT: u32 = 64; },
        quote! { enum Mode { On, Off } },
    ] {
        same(&passthrough(TokenStream::new(), item.clone()), &item);
    }
}

#[test]
fn arguments_are_refused_at_their_tokens() {
    let out = passthrough(quote! { ns = "Pad" }, quote! { fn f() {} }).to_string();
    assert!(out.contains("compile_error"), "{out}");
    assert!(out.contains("takes no arguments"), "{out}");
}

#[test]
fn text_that_is_not_an_item_is_refused() {
    let out = passthrough(TokenStream::new(), quote! { 1 + }).to_string();
    assert!(out.contains("compile_error"), "{out}");
}
