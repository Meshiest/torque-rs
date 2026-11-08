use super::*;
use quote::quote;

fn engine() -> syn::Path {
    syn::parse_quote!(::torque_engine)
}

fn class_of(input: TokenStream) -> String {
    derive_class(input, &engine()).to_string()
}

fn refused(out: &str, part: &str) {
    assert!(out.contains("compile_error"), "{out}");
    assert!(out.contains(part), "{out}");
}

#[test]
fn a_struct_becomes_a_class_with_a_declaration_for_each_field() {
    let out = derive_class(
        quote! {
            #[class(name = "RopeData", parent = "SimDataBlock")]
            struct RopeData {
                /// How many pieces.
                #[field(range = 2..=64, default = 8)]
                segments: i32,
                max_length: f32,
            }
        },
        &engine(),
    );
    let want = quote! {
        impl ::torque_engine::ScriptClass for RopeData {
            const NAME: &'static str = "RopeData";
            const PARENT: &'static str = "SimDataBlock";
            const FIELDS: &'static [::torque_engine::FieldDecl] = &[
                ::torque_engine::FieldDecl {
                    name: "segments",
                    field: "segments",
                    ty: <i32 as ::torque_engine::class::ClassField>::TYPE,
                    doc: "How many pieces.",
                    default: ::core::option::Option::Some("8"),
                    min: ::core::option::Option::Some((2) as f64),
                    max: ::core::option::Option::Some((64) as f64),
                    asset: ::core::option::Option::None,
                },
                ::torque_engine::FieldDecl {
                    name: "maxLength",
                    field: "max_length",
                    ty: <f32 as ::torque_engine::class::ClassField>::TYPE,
                    doc: "",
                    default: ::core::option::Option::None,
                    min: ::core::option::Option::None,
                    max: ::core::option::Option::None,
                    asset: ::core::option::Option::None,
                }
            ];
            fn from_values(
                get: &mut dyn ::core::ops::FnMut(
                    &::torque_engine::FieldDecl,
                ) -> ::core::option::Option<::torque_engine::class::FieldValue>,
            ) -> ::core::result::Result<Self, &'static str> {
                ::core::result::Result::Ok(Self {
                    segments: ::torque_engine::class::read_field::<i32>(
                        &<Self as ::torque_engine::ScriptClass>::FIELDS[0usize],
                        &mut *get,
                    )?,
                    max_length: ::torque_engine::class::read_field::<f32>(
                        &<Self as ::torque_engine::ScriptClass>::FIELDS[1usize],
                        &mut *get,
                    )?
                })
            }
        }
        const _: () = {
            let _: <i32 as ::torque_engine::class::ClassField>::Literal = 8;
            let _: ::core::ops::RangeInclusive<i32> = 2..=64;
            ::core::assert!(
                (2) as f64 <= (8) as f64 && (8) as f64 <= (64) as f64,
                "the default is outside the range"
            );
        };
        const _: () = {};
    };
    assert_eq!(out.to_string(), want.to_string());
}

#[test]
fn the_class_name_and_parent_default_to_the_struct_and_sim_object() {
    let out = class_of(quote! { struct Marker { a: i32 } });
    assert!(
        out.contains("const NAME : & 'static str = \"Marker\""),
        "{out}"
    );
    assert!(
        out.contains("const PARENT : & 'static str = \"SimObject\""),
        "{out}"
    );
    assert!(!out.contains("HAS_PRELOAD"), "{out}");
}

#[test]
fn a_preload_hook_is_called_with_the_object_and_the_script_engine() {
    let out = class_of(quote! {
        #[class(preload = RopeData::check)]
        struct RopeData { a: i32 }
    });
    assert!(out.contains("const HAS_PRELOAD : bool = true"), "{out}");
    assert!(out.contains("RopeData :: check (self , vm , id)"), "{out}");
}

#[test]
fn field_names_and_docs_can_be_given() {
    let out = class_of(quote! {
        struct C {
            /// ignored
            #[field(name = "texture", doc = "the picture")]
            material: Filename,
        }
    });
    assert!(out.contains("name : \"texture\""), "{out}");
    assert!(out.contains("field : \"material\""), "{out}");
    assert!(out.contains("doc : \"the picture\""), "{out}");
}

#[test]
fn a_filename_field_can_name_an_asset_type() {
    let out = class_of(quote! {
        struct C {
            #[field(asset = my_game::Words)]
            list: Filename,
        }
    });
    assert!(
        out.contains(
            "asset : :: core :: option :: Option :: Some (:: torque_engine :: assets :: AssetType :: of :: < my_game :: Words > ())"
        ),
        "{out}"
    );
    assert!(
        out.contains("names_an_asset :: < Filename >"),
        "the field's type is checked to be a Filename: {out}"
    );
}

#[test]
fn the_mistakes_are_refused_at_their_tokens() {
    refused(
        &class_of(quote! { #[class(kind = "x")] struct C { a: i32 } }),
        "expected `name",
    );
    refused(
        &class_of(quote! { #[class(name = "9x")] struct C { a: i32 } }),
        "a class name is",
    );
    refused(
        &class_of(quote! { #[class(parent = "a b")] struct C { a: i32 } }),
        "a class name is",
    );
    refused(
        &class_of(quote! { struct C { #[field(colour = 1)] a: i32 } }),
        "expected `default",
    );
    refused(
        &class_of(quote! { struct C { #[field(range = 1..5)] a: i32 } }),
        "`low..=high`",
    );
    refused(
        &class_of(quote! { struct C { #[field(default = 1 + 1)] a: i32 } }),
        "a default is a literal",
    );
    refused(
        &class_of(quote! { struct C { #[field(name = "a b")] a: i32 } }),
        "a field's name",
    );
    refused(&class_of(quote! { struct C(i32); }), "named fields");
    refused(&class_of(quote! { struct C<T> { a: T } }), "generic");
}

#[test]
fn two_fields_with_one_console_name_are_refused() {
    let out = class_of(quote! {
        struct C {
            max_length: f32,
            #[field(name = "MAXLENGTH")] other: f32,
        }
    });
    refused(&out, "is also the console field of `max_length`");
}
