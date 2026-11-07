use super::*;
use proc_macro_crate::FoundCrate;
use quote::quote;

/// The engine crate's path when the deriving crate depends on `torque-engine`.
fn engine() -> syn::Path {
    syn::parse_quote!(::torque_engine)
}

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

#[test]
fn a_plugin_id_defaults_to_the_package_and_type() {
    let out = derive_plugin_id(quote! { struct HudPlugin; }, "my-game", &engine());
    let want = quote! {
        impl ::torque_engine::PluginType for HudPlugin {
            const ID: ::torque_engine::PluginId = ::torque_engine::PluginId::new("my_game::HudPlugin");
        }
    };
    same(&out, &want);
}

#[test]
fn a_plugin_id_and_crate_path_can_be_given() {
    let out = derive_plugin_id(
        quote! {
            /// Pads.
            #[derive(Default)]
            #[plugin(id = "torque.gamepad", crate = torque::engine)]
            pub struct Gamepad { dead_zone: f32 }
        },
        "my-game",
        &engine(),
    );
    let want = quote! {
        impl torque::engine::PluginType for Gamepad {
            const ID: torque::engine::PluginId = torque::engine::PluginId::new("torque.gamepad");
        }
    };
    same(&out, &want);
}

#[test]
fn default_ids_keep_only_id_characters() {
    assert_eq!(default_id("my-game", "HudPlugin"), "my_game::HudPlugin");
    assert_eq!(default_id("my_game2", "Rope"), "my_game2::Rope");
    assert_eq!(default_id("caf\u{e9}", "R\u{f6}pe"), "caf_::R_pe");
}

#[test]
fn a_bad_plugin_id_is_refused_at_its_tokens() {
    for id in ["", "torque gamepad", "torque/pad"] {
        let out =
            derive_plugin_id(quote! { #[plugin(id = #id)] struct P; }, "x", &engine()).to_string();
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("a plugin id is"), "{out}");
    }
}

#[test]
fn unknown_keys_generics_and_no_package_are_refused() {
    let unknown =
        derive_plugin_id(quote! { #[plugin(name = "x")] struct P; }, "x", &engine()).to_string();
    assert!(unknown.contains("expected `id"), "{unknown}");
    let generic = derive_plugin_id(quote! { struct P<T>(T); }, "x", &engine()).to_string();
    assert!(generic.contains("one per type"), "{generic}");
    let nameless = derive_plugin_id(quote! { struct P; }, "", &engine()).to_string();
    assert!(nameless.contains("CARGO_PKG_NAME"), "{nameless}");
    // With an id given, no package name is needed.
    let given =
        derive_plugin_id(quote! { #[plugin(id = "p")] struct P; }, "", &engine()).to_string();
    assert!(!given.contains("compile_error"), "{given}");
}

#[test]
fn other_attributes_are_left_alone() {
    let out = derive_plugin_id(
        quote! { #[serde(rename = "x")] struct P; },
        "pkg",
        &engine(),
    )
    .to_string();
    assert!(out.contains("\"pkg::P\""), "{out}");
}

#[test]
fn the_engine_path_follows_the_dependencies() {
    let path = |engine, umbrella| engine_path(engine, umbrella).to_token_stream().to_string();
    let name = |n: &str| Some(FoundCrate::Name(n.into()));
    // `torque-engine` itself, or as renamed in the manifest; it wins over `torque`.
    assert_eq!(
        path(name("torque_engine"), name("torque")),
        ":: torque_engine"
    );
    assert_eq!(path(name("engine"), None), ":: engine");
    // Only `torque`: its `engine` module.
    assert_eq!(path(None, name("torque")), ":: torque :: engine");
    assert_eq!(path(None, name("my_torque")), ":: my_torque :: engine");
    assert_eq!(path(None, Some(FoundCrate::Itself)), ":: torque :: engine");
    // The engine crate itself, and a crate with neither.
    assert_eq!(path(Some(FoundCrate::Itself), None), ":: torque_engine");
    assert_eq!(path(None, None), ":: torque_engine");
}

#[test]
fn the_derive_names_the_engine_by_the_path_found() {
    let out = derive_plugin_id(
        quote! { struct Rope; },
        "game",
        &syn::parse_quote!(::torque::engine),
    );
    let want = quote! {
        impl ::torque::engine::PluginType for Rope {
            const ID: ::torque::engine::PluginId = ::torque::engine::PluginId::new("game::Rope");
        }
    };
    same(&out, &want);
}
