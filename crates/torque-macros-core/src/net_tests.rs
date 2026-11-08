use super::*;
use quote::quote;

fn engine() -> syn::Path {
    syn::parse_quote!(::torque_engine)
}

fn same(a: &TokenStream, b: &TokenStream) {
    assert_eq!(a.to_string(), b.to_string());
}

/// The error a refused expansion carries.
fn refused(out: &TokenStream, part: &str) {
    let out = out.to_string();
    assert!(out.contains("compile_error"), "{out}");
    assert!(out.contains(part), "{out}");
}

#[test]
fn a_ghost_record_becomes_its_layout_its_record_and_its_packer() {
    let out = derive_ghost_record(
        quote! {
            struct RopeGhost {
                #[net(bits = 6)]
                segment_count: u32,
                #[net(mask = 0x2, name = "tint")]
                color: Option<[f32; 3]>,
                #[net(skip)]
                local: u8,
            }
        },
        &engine(),
    );
    let want = quote! {
        impl ::torque_engine::__ghosts::codec::Layout for RopeGhost {
            fn layout<C: ::torque_engine::__ghosts::codec::Codec>(&mut self, c: &mut C) -> ::torque_engine::__ghosts::codec::R {
                let _ = &c;
                ::torque_engine::__ghosts::plugin::NetField::layout(&mut self.segment_count, c, "segmentCount",
                    ::torque_engine::__ghosts::plugin::FieldSpec { bits: 6u8, max_len: 0usize })?;
                ::torque_engine::__ghosts::plugin::NetField::layout(&mut self.color, c, "tint",
                    ::torque_engine::__ghosts::plugin::FieldSpec { bits: 0u8, max_len: 0usize })?;
                ::core::result::Result::Ok(())
            }
        }
        impl ::torque_engine::__ghosts::plugin::NetRecord for RopeGhost {
            fn merge(&mut self, sent: &Self) {
                let _ = sent;
                ::torque_engine::__ghosts::plugin::NetField::merge_from(&mut self.segment_count, &sent.segment_count);
                ::torque_engine::__ghosts::plugin::NetField::merge_from(&mut self.color, &sent.color);
            }
            fn datablock(&self) -> ::core::option::Option<u32> {
                ::core::option::Option::None
                    .or_else(|| ::torque_engine::__ghosts::plugin::NetField::datablock(&self.segment_count))
                    .or_else(|| ::torque_engine::__ghosts::plugin::NetField::datablock(&self.color))
            }
            fn transform(&self) -> ::core::option::Option<::torque_engine::__ghosts::codec::Xf> {
                ::core::option::Option::None
                    .or_else(|| ::torque_engine::__ghosts::plugin::NetField::transform(&self.segment_count))
                    .or_else(|| ::torque_engine::__ghosts::plugin::NetField::transform(&self.color))
            }
            fn script_fields(&self) -> ::std::vec::Vec<(::std::string::String, u32, ::torque_engine::__ghosts::Value)> {
                let mut out = ::std::vec::Vec::new();
                ::torque_engine::__ghosts::plugin::push_values(&mut out, "segmentCount", &self.segment_count);
                ::torque_engine::__ghosts::plugin::push_values(&mut out, "tint", &self.color);
                out
            }
        }
        impl ::torque_engine::__ghosts::plugin::GhostRecord for RopeGhost {
            fn pack(
                view: &dyn ::torque_engine::__ghosts::ObjectView,
                mask: u32,
                ctx: &::torque_engine::__ghosts::PackCtx,
            ) -> (Self, u32) {
                let _ = (mask, ctx);
                (Self {
                    segment_count: <u32 as ::torque_engine::__ghosts::plugin::NetField>::from_view(view, "segmentCount", 0,
                        ::torque_engine::__ghosts::plugin::FieldSpec { bits: 6u8, max_len: 0usize }),
                    color: if mask & ((0x2) as u32) != 0 {
                        <Option<[f32; 3]> as ::torque_engine::__ghosts::plugin::NetField>::from_view(view, "tint", 0,
                            ::torque_engine::__ghosts::plugin::FieldSpec { bits: 0u8, max_len: 0usize })
                    } else {
                        ::core::default::Default::default()
                    },
                    local: ::core::default::Default::default()
                }, 0)
            }
        }
        const _: () = {
            ::core::assert!(6u8 <= <u32 as ::torque_engine::__ghosts::plugin::NetField>::MAX_BITS,
                "`bits = 6` is more than the field's type takes");
            ::core::assert!(<Option<[f32; 3]> as ::torque_engine::__ghosts::plugin::NetField>::OPTIONAL,
                "`color` has a `mask`, so it must be an `Option` (none while the mask is clear)");
        };
    };
    same(&out, &want);
}

#[test]
fn a_datablock_record_packs_from_the_datablocks_view_and_checks_its_caps() {
    let out = derive_datablock_codec(
        quote! {
            #[net(crate = ::torque::engine)]
            struct PropData {
                #[net(max_len = 64)]
                model: String,
            }
        },
        &engine(),
    );
    let text = out.to_string();
    assert!(
        text.contains(
            "impl :: torque :: engine :: __ghosts :: plugin :: DatablockCodec for PropData"
        ),
        "{text}"
    );
    assert!(
        text.contains("fn pack (view : & dyn :: torque :: engine :: __ghosts :: ObjectView , widths : :: torque :: engine :: __ghosts :: codec :: NetWidths) -> Self"),
        "{text}"
    );
    assert!(
        text.contains("64usize <= < String as :: torque :: engine :: __ghosts :: plugin :: NetField > :: MAX_LEN"),
        "{text}"
    );
    assert!(!text.contains("GhostRecord"), "{text}");
}

#[test]
fn a_console_classs_field_name_is_the_one_the_packer_reads() {
    let out = derive_datablock_codec(
        quote! {
            struct PropData {
                #[field(name = "partCount", range = 0..=8, default = 1)]
                parts: i32,
                #[field(range = 0.0..=1.0)]
                #[net(name = "own")]
                mine: f32,
            }
        },
        &engine(),
    )
    .to_string();
    assert!(out.contains("from_view (view , \"partCount\""), "{out}");
    assert!(out.contains("from_view (view , \"own\""), "{out}");
}

#[test]
fn a_struct_without_widths_or_caps_checks_nothing() {
    let out = derive_ghost_record(quote! { struct Plain { on: bool } }, &engine()).to_string();
    assert!(!out.contains("const _"), "{out}");
}

#[test]
fn what_the_derives_refuse() {
    let ghost = |input| derive_ghost_record(input, &engine());
    refused(
        &ghost(quote! { struct T { #[net(bits = 0)] a: u32 } }),
        "`bits` is 1 to 32",
    );
    refused(
        &ghost(quote! { struct T { #[net(bits = 33)] a: u32 } }),
        "`bits` is 1 to 32",
    );
    refused(
        &ghost(quote! { struct T { #[net(max_len = 0)] a: String } }),
        "`max_len` is at least 1",
    );
    refused(
        &ghost(quote! { struct T { #[net(width = 3)] a: u32 } }),
        "expected `bits = N`",
    );
    refused(
        &ghost(quote! { struct T { #[net(name = "2d")] a: u32 } }),
        "a field's name",
    );
    refused(
        &ghost(quote! { struct T { #[net(skip, bits = 3)] a: u32 } }),
        "a `skip` field is not sent",
    );
    refused(
        &ghost(
            quote! { struct T { a_b: u32, #[net(name = "ABB")] c: u32, #[net(name = "aB")] d: u32 } },
        ),
        "is also the field name of `a_b`",
    );
    refused(
        &ghost(quote! { struct T(u32); }),
        "a struct with named fields",
    );
    refused(
        &ghost(quote! { struct T<X> { a: X } }),
        "a generic struct cannot be one",
    );
    refused(
        &ghost(quote! { #[net(prefix = "x")] struct T { a: u32 } }),
        "expected `crate = path`",
    );
    refused(
        &derive_datablock_codec(
            quote! { struct T { #[net(mask = 1)] a: Option<u32> } },
            &engine(),
        ),
        "a datablock goes out whole",
    );
}
