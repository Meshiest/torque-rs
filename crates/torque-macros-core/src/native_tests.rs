use super::*;

/// The engine crate's path when the crate depends on `torque-engine`.
fn engine() -> syn::Path {
    syn::parse_quote!(::torque_engine)
}

/// Token streams compare by their text, which is spaced the same way for both sides.
fn same(a: &TokenStream, b: &TokenStream) {
    assert_eq!(a.to_string(), b.to_string());
}

fn function_of(args: TokenStream, item: TokenStream) -> String {
    function(args, item, &engine()).to_string()
}

fn methods_of(args: TokenStream, item: TokenStream) -> String {
    methods(args, item, &engine()).to_string()
}

/// The error a refused expansion carries.
fn refused(out: &str, part: &str) {
    assert!(out.contains("compile_error"), "{out}");
    assert!(out.contains(part), "{out}");
}

#[test]
fn rust_names_become_camel_case() {
    assert_eq!(camel_case("dead_zone"), "deadZone");
    assert_eq!(camel_case("set_dead_zone"), "setDeadZone");
    assert_eq!(camel_case("x"), "x");
    assert_eq!(camel_case("_rest"), "rest");
    assert_eq!(camel_case("token_variable"), "tokenVariable");
    assert_eq!(camel_case("x_y_z"), "xYZ");
    assert_eq!(camel_case("a__b_"), "aB");
    assert_eq!(camel_case("_"), "");
    assert_eq!(camel_case("Already"), "Already");
}

#[test]
fn the_doc_comment_gives_the_description_and_is_kept_whole() {
    let f: syn::ItemFn = syn::parse_quote! {
        /// Sets the stick dead zone (0 to 0.9).
        ///
        /// Returns the value kept.
        #[inline]
        fn f() {}
    };
    let docs = doc_text(&f.attrs);
    assert_eq!(
        docs,
        "Sets the stick dead zone (0 to 0.9).\n\nReturns the value kept."
    );
    assert_eq!(first_line(&docs), "Sets the stick dead zone (0 to 0.9).");
    assert_eq!(first_line(""), "");
    let block: syn::ItemFn = syn::parse_quote! {
        /**
           The first line.
           The second.
        */
        fn f() {}
    };
    assert_eq!(first_line(&doc_text(&block.attrs)), "The first line.");
}

#[test]
fn a_function_is_kept_with_its_declaration_and_check_beside_it() {
    let out = function(
        quote!(ns = "Pad", name = "setDeadZone"),
        quote! {
            /// sets the zone
            pub fn set_dead_zone(mut zone_kept: ResMut<'_, DeadZone>, zone: f32) -> f32 { zone }
        },
        &engine(),
    );
    let want = quote! {
        /// sets the zone
        pub fn set_dead_zone(mut zone_kept: ResMut<'_, DeadZone>, zone: f32) -> f32 { zone }

        #[doc = "The declaration of the native `set_dead_zone`, for `natives!`."]
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        pub struct set_dead_zone {}

        impl ::torque_engine::script::Natives for set_dead_zone {
            const NATIVES: &'static [::torque_engine::script::NativeDecl] = &[::torque_engine::script::NativeDecl {
                namespace: "Pad",
                name: "setDeadZone",
                method: false,
                dev: false,
                params: &["zoneKept", "zone"],
                description: "sets the zone",
                usage: ::core::option::Option::None,
                docs: "sets the zone",
                effects: ::torque_engine::Effects::NONE,
                native: {
                    fn __torque_native() -> ::torque_engine::script::TypedNative {
                        ::torque_engine::IntoNative::into_native(set_dead_zone)
                    }
                    __torque_native
                },
            }];
        }

        const _: () = match ::torque_engine::param::native_shape_error(&[
            <ResMut<'_, DeadZone> as ::torque_engine::Param>::KIND,
            <f32 as ::torque_engine::Param>::KIND
        ]) {
            ::core::option::Option::Some(error) => ::core::panic!("{}", error),
            ::core::option::Option::None => {}
        };
    };
    same(&out, &want);
}

#[test]
fn the_name_defaults_to_camel_case_and_the_namespace_to_the_global_one() {
    let out = function_of(TokenStream::new(), quote! { fn get_words(text: &str) {} });
    assert!(out.contains("namespace : \"\""), "{out}");
    assert!(out.contains("name : \"getWords\""), "{out}");
    assert!(out.contains("description : \"\""), "{out}");
    assert!(out.contains("params : & [\"text\"]"), "{out}");
    let raw = function_of(TokenStream::new(), quote! { fn r#type(r#in: i32) {} });
    assert!(raw.contains("name : \"type\""), "{raw}");
    assert!(raw.contains("params : & [\"in\"]"), "{raw}");
}

#[test]
fn argument_names_can_be_given_and_their_attributes_go() {
    let out = function_of(
        TokenStream::new(),
        quote! { fn f(#[arg(name = "who")] person: &str, _: i32, (a, b): (i32, i32), _unused: bool) {} },
    );
    assert!(
        out.contains("params : & [\"who\" , \"arg2\" , \"arg3\" , \"unused\"]"),
        "{out}"
    );
    assert!(!out.contains("# [arg"), "the attribute is taken off: {out}");
}

#[test]
fn usage_description_and_dev_are_options() {
    let out = function_of(
        quote!(usage = "setRenderer(id);", description = "", dev),
        quote! {
            /// Switches the renderer.
            fn set_renderer(id: &str) -> bool { true }
        },
    );
    assert!(
        out.contains("usage : :: core :: option :: Option :: Some (\"setRenderer(id);\")"),
        "{out}"
    );
    assert!(out.contains("description : \"\""), "{out}");
    assert!(out.contains("dev : true"), "{out}");
    assert!(out.contains("docs : \"Switches the renderer.\""), "{out}");
}

#[test]
fn the_check_names_types_without_their_lifetimes() {
    let out = function_of(
        TokenStream::new(),
        quote! { fn f<'a>(text: &'a str, words: Rest<'a>, kept: &'static str) {} },
    );
    assert!(
        out.contains("< & '_ str as :: torque_engine :: Param >"),
        "{out}"
    );
    assert!(
        out.contains("< Rest < '_ > as :: torque_engine :: Param >"),
        "{out}"
    );
    assert!(out.contains("< & 'static str as"), "{out}");
}

#[test]
fn the_crate_path_can_be_given() {
    let out = function_of(quote!(crate = torque::engine), quote! { fn f() {} });
    assert!(
        out.contains("impl torque :: engine :: script :: Natives for f"),
        "{out}"
    );
    assert!(!out.contains(":: torque_engine"), "{out}");
}

#[test]
fn what_is_no_free_function_is_refused_at_its_tokens() {
    refused(
        &function_of(TokenStream::new(), quote! { fn f(&self, a: i32) {} }),
        "`self` in a free function",
    );
    refused(
        &function_of(TokenStream::new(), quote! { trait Tool { fn f(&self); } }),
        "goes on a free function",
    );
    refused(
        &function_of(TokenStream::new(), quote! { impl Tool { fn f() {} } }),
        "`#[methods(class",
    );
    refused(
        &function_of(TokenStream::new(), quote! { struct S; }),
        "goes on a free function",
    );
    refused(
        &function_of(TokenStream::new(), quote! { fn f<T: Copy>(t: T) {} }),
        "not generic",
    );
    refused(
        &function_of(TokenStream::new(), quote! { async fn f() {} }),
        "not `async`",
    );
    refused(
        &function_of(TokenStream::new(), quote! { fn f(t: impl Copy) {} }),
        "concrete type",
    );
}

#[test]
fn bad_options_are_refused_at_their_tokens() {
    refused(
        &function_of(quote!(namespace = "Pad"), quote! { fn f() {} }),
        "expected `ns",
    );
    refused(
        &function_of(quote!(name = "set zone"), quote! { fn f() {} }),
        "a script name is",
    );
    refused(
        &function_of(quote!(name = "2d"), quote! { fn f() {} }),
        "a script name is",
    );
    refused(
        &function_of(
            TokenStream::new(),
            quote! { fn f(#[arg(title = "x")] a: i32) {} },
        ),
        "expected `name",
    );
    refused(
        &function_of(
            TokenStream::new(),
            quote! { fn f(#[arg(name = "")] a: i32) {} },
        ),
        "not empty",
    );
}

#[test]
fn every_pub_fn_of_a_block_is_a_method_but_the_skipped() {
    let out = methods_of(
        quote!(class = "Player"),
        quote! {
            impl Tools {
                /// whether it is armed
                #[torque(name = "isArmed")]
                pub fn armed(this: This) -> bool { true }
                pub fn give_ammo(amount: i32, max: Option<i32>) -> i32 { amount }
                #[torque(skip)]
                pub fn helper(#[arg(name = "x")] x: i32) {}
                fn private() {}
            }
        },
    );
    assert!(
        out.contains("impl :: torque_engine :: script :: Natives for Tools"),
        "{out}"
    );
    assert!(out.contains("name : \"isArmed\""), "{out}");
    assert!(
        out.contains("description : \"whether it is armed\""),
        "{out}"
    );
    assert!(out.contains("name : \"giveAmmo\""), "{out}");
    assert!(out.contains("namespace : \"Player\""), "{out}");
    assert!(out.contains("method : true"), "{out}");
    assert!(out.contains("into_native (< Tools > :: armed)"), "{out}");
    assert!(!out.contains("\"helper\""), "{out}");
    assert!(!out.contains("\"private\""), "{out}");
    assert!(
        !out.contains("# [torque"),
        "the attributes are taken off: {out}"
    );
    assert!(
        !out.contains("# [arg"),
        "skipped fns lose theirs too: {out}"
    );
}

#[test]
fn a_receiver_is_the_objects_data_and_the_vm_is_handed_on() {
    let out = methods_of(
        TokenStream::new(),
        quote! {
            impl Weld {
                pub fn heat(&mut self, vm: VmMut, by: f32, other: Option<Self>) -> f32 { by }
            }
        },
    );
    assert!(
        out.contains("namespace : \"Weld\""),
        "the class defaults to the type: {out}"
    );
    assert!(
        out.contains("params : & [\"this\" , \"vm\" , \"by\" , \"other\"]"),
        "the wrapper's parameters are named: {out}"
    );
    assert!(
        out.contains(":: torque_engine :: script :: with_object_data"),
        "{out}"
    );
    assert!(
        out.contains("< Weld > :: heat (__torque_data , :: torque_engine :: VmMut :: new (__torque_vm) , __torque_arg1 , __torque_arg2)"),
        "{out}"
    );
    assert!(
        out.contains("__torque_arg2 : Option < Weld >"),
        "`Self` is the type outside the block: {out}"
    );
    assert!(
        out.contains("< Option < Weld > as :: torque_engine :: Param >"),
        "{out}"
    );
}

#[test]
fn a_trait_impl_declares_every_fn_and_a_list_type_can_be_named() {
    let out = methods_of(
        quote!(class = "Weld", list = WeldTool),
        quote! {
            impl Tool for Weld {
                fn power(&self) -> i32 { 1 }
            }
        },
    );
    assert!(out.contains("pub struct WeldTool { }"), "{out}");
    assert!(
        out.contains("impl :: torque_engine :: script :: Natives for WeldTool"),
        "{out}"
    );
    assert!(out.contains("< Weld as Tool > :: power"), "{out}");
    assert!(out.contains("name : \"power\""), "{out}");
}

#[test]
fn what_methods_cannot_declare_is_refused_at_its_tokens() {
    refused(
        &methods_of(TokenStream::new(), quote! { trait Tool { fn f(&self); } }),
        "not on a trait definition",
    );
    refused(
        &methods_of(TokenStream::new(), quote! { fn f() {} }),
        "a free function takes `#[function]`",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl<T> Wrap<T> { pub fn f() {} } },
        ),
        "concrete type",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl Tools { pub fn f(self) {} } },
        ),
        "`&self` or `&mut self`",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl Tools { pub fn f(self: Box<Self>) {} } },
        ),
        "`&self` or `&mut self`",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl Tools { pub fn is_armed() {} #[torque(name = "ISARMED")] pub fn other() {} } },
        ),
        "two methods of this block are called `ISARMED` (`isArmed` came first",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl Tools { #[torque(skip)] fn f() {} } },
        ),
        "only `pub` fns",
    );
    refused(
        &methods_of(
            TokenStream::new(),
            quote! { impl Tools { #[torque(dev)] pub fn f() {} } },
        ),
        "not a development command",
    );
    refused(
        &methods_of(quote!(klass = "X"), quote! { impl Tools {} }),
        "expected `class",
    );
    refused(
        &methods_of(quote!(class = "Bad Class"), quote! { impl Tools {} }),
        "a class name is",
    );
}
