use super::*;
use quote::quote;

fn engine() -> syn::Path {
    syn::parse_quote!(::torque_engine)
}

fn same(a: &TokenStream, b: &TokenStream) {
    assert_eq!(a.to_string(), b.to_string());
}

fn prefs_of(input: TokenStream) -> String {
    derive_prefs(input, &engine()).to_string()
}

/// The error a refused expansion carries.
fn refused(out: &str, part: &str) {
    assert!(out.contains("compile_error"), "{out}");
    assert!(out.contains(part), "{out}");
}

#[test]
fn a_struct_of_prefs_becomes_an_impl_with_a_declaration_for_each() {
    let out = derive_prefs(
        quote! {
            #[prefs(prefix = "$Pref::Pad")]
            struct PadPrefs {
                /// How far the stick must move.
                #[pref(default = 0.15, range = 0.0..=0.9)]
                dead_zone: f32,
                #[pref(default = true)]
                invert_y: bool,
            }
        },
        &engine(),
    );
    let want = quote! {
        impl ::torque_engine::Prefs for PadPrefs {
            const PREFIX: &'static str = "$Pref::Pad";
            const PREFS: &'static [::torque_engine::PrefDecl] = &[
                ::torque_engine::PrefDecl {
                    name: "$Pref::Pad::DeadZone",
                    field: "dead_zone",
                    kind: <f32 as ::torque_engine::prefs::PrefType>::KIND,
                    doc: "How far the stick must move.",
                    default: "0.15",
                    min: ::core::option::Option::Some((0.0) as f64),
                    max: ::core::option::Option::Some((0.9) as f64),
                },
                ::torque_engine::PrefDecl {
                    name: "$Pref::Pad::InvertY",
                    field: "invert_y",
                    kind: <bool as ::torque_engine::prefs::PrefType>::KIND,
                    doc: "",
                    default: "1",
                    min: ::core::option::Option::None,
                    max: ::core::option::Option::None,
                }
            ];
            fn read_with(
                read: &mut dyn ::core::ops::FnMut(
                    &::torque_engine::PrefDecl,
                ) -> ::core::option::Option<::std::vec::Vec<u8>>,
            ) -> Self {
                Self {
                    dead_zone: ::torque_engine::prefs::read_pref::<f32>(
                        &<Self as ::torque_engine::Prefs>::PREFS[0usize],
                        &mut *read,
                    ),
                    invert_y: ::torque_engine::prefs::read_pref::<bool>(
                        &<Self as ::torque_engine::Prefs>::PREFS[1usize],
                        &mut *read,
                    )
                }
            }
        }
        const _: () = {
            let _: <f32 as ::torque_engine::prefs::PrefType>::Literal = 0.15;
            let _: ::core::ops::RangeInclusive<f32> = 0.0..=0.9;
            ::core::assert!(
                (0.0) as f64 <= (0.15) as f64 && (0.15) as f64 <= (0.9) as f64,
                "the default is outside the range"
            );
        };
        const _: () = {
            let _: <bool as ::torque_engine::prefs::PrefType>::Literal = true;
        };
    };
    same(&out, &want);
}

#[test]
fn defaults_become_the_text_a_script_holds() {
    let default_of = |default: TokenStream| {
        let out = prefs_of(quote! {
            #[prefs(prefix = "$Pref::X")]
            struct P { #[pref(default = #default)] v: i32 }
        });
        let start = out.find("default : \"").expect("a default") + "default : \"".len();
        out[start..start + out[start..].find('"').expect("its end")].to_string()
    };
    assert_eq!(default_of(quote!(5)), "5");
    assert_eq!(default_of(quote!(-5)), "-5");
    assert_eq!(default_of(quote!(1_000)), "1000");
    assert_eq!(default_of(quote!(2.5f32)), "2.5");
    assert_eq!(default_of(quote!(-0.25)), "-0.25");
    assert_eq!(default_of(quote!(true)), "1");
    assert_eq!(default_of(quote!(false)), "0");
    assert_eq!(default_of(quote!("hello")), "hello");
}

#[test]
fn a_name_and_a_doc_can_be_given() {
    let out = prefs_of(quote! {
        #[prefs(prefix = "$Pref::Pad")]
        struct P {
            /// ignored
            #[pref(default = 1, name = "Profile", doc = "which one")]
            active_profile: u32,
        }
    });
    assert!(out.contains("name : \"$Pref::Pad::Profile\""), "{out}");
    assert!(out.contains("doc : \"which one\""), "{out}");
}

#[test]
fn field_names_become_pascal_case() {
    assert_eq!(pascal_case("dead_zone"), "DeadZone");
    assert_eq!(pascal_case("x"), "X");
    assert_eq!(pascal_case("_a_b"), "AB");
}

#[test]
fn a_prefix_must_be_a_global_name_prefix() {
    for bad in [
        "Pref::Pad",
        "$",
        "$Pref::",
        "$Pref::Pad Mode",
        "$1abc",
        "$Pref:Pad",
    ] {
        let out =
            prefs_of(quote! { #[prefs(prefix = #bad)] struct P { #[pref(default = 1)] a: i32 } });
        refused(&out, "a prefix is");
    }
    for good in ["$Pref", "$Pref::Pad", "$Server::Motd2"] {
        let out =
            prefs_of(quote! { #[prefs(prefix = #good)] struct P { #[pref(default = 1)] a: i32 } });
        assert!(!out.contains("compile_error"), "{good}: {out}");
    }
}

#[test]
fn the_mistakes_are_refused_at_their_tokens() {
    let one = |attr: TokenStream, field: TokenStream| {
        prefs_of(quote! { #[prefs(prefix = "$Pref::X")] #attr struct P { #field a: i32 } })
    };
    // No prefix.
    refused(
        &prefs_of(quote! { struct P { #[pref(default = 1)] a: i32 } }),
        "give the prefix",
    );
    // An unknown key at the struct and at a field.
    refused(
        &prefs_of(
            quote! { #[prefs(prefix = "$P", colour = 1)] struct P { #[pref(default = 1)] a: i32 } },
        ),
        "expected `prefix",
    );
    refused(
        &one(quote!(), quote!(#[pref(default = 1, speed = 2)])),
        "expected `default",
    );
    // No default, a default that is not a literal, a range that is not closed.
    refused(
        &one(quote!(), quote!(#[pref(range = 0..=5)])),
        "needs a default",
    );
    refused(
        &one(quote!(), quote!(#[pref(default = 1 + 2)])),
        "a default is a literal",
    );
    refused(
        &one(quote!(), quote!(#[pref(default = LIMIT)])),
        "a default is a literal",
    );
    refused(
        &one(quote!(), quote!(#[pref(default = 1, range = 0..5)])),
        "`low..=high`",
    );
    refused(
        &one(quote!(), quote!(#[pref(default = 1, range = ..=5)])),
        "`low..=high`",
    );
    // A bad name.
    refused(
        &one(quote!(), quote!(#[pref(default = 1, name = "9a")])),
        "a pref's name",
    );
}

#[test]
fn only_a_struct_of_named_fields_without_generics_is_a_set_of_prefs() {
    refused(
        &prefs_of(quote! { #[prefs(prefix = "$P")] struct P(i32); }),
        "named fields",
    );
    refused(
        &prefs_of(quote! { #[prefs(prefix = "$P")] enum P { A } }),
        "named fields",
    );
    refused(
        &prefs_of(quote! { #[prefs(prefix = "$P")] struct P<T> { a: T } }),
        "generic",
    );
}

#[test]
fn two_fields_with_one_global_are_refused() {
    let out = prefs_of(quote! {
        #[prefs(prefix = "$P")]
        struct P {
            #[pref(default = 1)] dead_zone: i32,
            #[pref(default = 1, name = "DEADZONE")] other: i32,
        }
    });
    refused(&out, "is also the global of `dead_zone`");
}

#[test]
fn the_engine_crate_can_be_named() {
    let out = prefs_of(quote! {
        #[prefs(prefix = "$P", crate = torque::engine)]
        struct P { #[pref(default = 1)] a: i32 }
    });
    assert!(
        out.contains("impl torque :: engine :: Prefs for P"),
        "{out}"
    );
}
