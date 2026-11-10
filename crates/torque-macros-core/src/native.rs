//! `#[function]` and `#[methods]`: Rust functions as TorqueScript natives, declared as constants the engine's script
//! registry takes (`ScriptRegistry::natives`, listed with `natives![...]`).
//!
//! The attributes read what only the source has: the parameters' names (camelCase for the usage text, or
//! `#[arg(name = "...")]`), the doc comment (its first line is the usage text's description), the script name and
//! namespace. What the engine knows by type stays with the engine: which parameters are script arguments and which
//! are injected is `IntoNative`'s split, made from the same types. A signature the engine cannot run is refused when
//! the crate is checked, by a constant the expansion adds (the parameters' kinds, checked by the engine's own rule).

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Attribute, FnArg, ImplItem, Item, LitStr, Pat, Signature, Type, Visibility};

/// The expansion of `#[function(...)]` on a free function, with `engine` the engine crate's path (`find_engine`).
///
/// The attribute takes `ns = "Pad"` (the namespace; the global one when left out), `name = "setDeadZone"` (the name
/// scripts call; the function's name in camelCase when left out), `usage = "..."` (a usage text kept exactly, in
/// place of the generated one), `description = "..."` (in place of the doc comment's first line; empty for none),
/// `dev` (a development command), `effects = "files+code"` (what it does beyond its result: `files`, `code`, `engine`,
/// `network`, `permissions`, `variables`, joined with `+`; see the engine's `Effects`) and `crate = path` (the engine crate's path).
///
/// The function is kept as written, without the `#[arg(...)]` attributes on its parameters. Beside it come a hidden
/// type of the same name (types and functions have separate names, so `natives![set_dead_zone]` finds it) that
/// implements `Natives` with the declaration, and a constant that checks the parameter kinds. The hidden type collides
/// with a type, module or import of the function's name in the same scope (E0428); the engine's docs say so.
pub fn function(args: TokenStream, item: TokenStream, engine: &syn::Path) -> TokenStream {
    expand_function(args, item, engine).unwrap_or_else(syn::Error::into_compile_error)
}

/// The expansion of `#[methods(...)]` on an impl block, with `engine` the engine crate's path.
///
/// The attribute takes `class = "Player"` (the class the methods are called on; the type's name when left out),
/// `list = Name` (the type that implements `Natives`, made here; the block's own type when left out, which allows one
/// `#[methods]` block per type) and `crate = path`. Every `pub fn` of an inherent block becomes a method, and every
/// fn of a trait impl (whose fns have the trait's visibility); `#[torque(skip)]` leaves one out, and
/// `#[torque(name = "...", usage = "...", description = "...")]` sets what `#[function]` sets.
///
/// A method that takes `&self` or `&mut self` is called on the object's Rust data (`with_object_data`): its
/// `SimObject::ext`, which must be the block's type. One without a receiver is called as it is; `This` gives it the
/// object's id.
pub fn methods(args: TokenStream, item: TokenStream, engine: &syn::Path) -> TokenStream {
    expand_methods(args, item, engine).unwrap_or_else(syn::Error::into_compile_error)
}

/// `snake_case` as `camelCase`: the words after the first start with a capital; leading underscores (an unused
/// parameter's) go. `dead_zone` -> `deadZone`, `x` -> `x`, `_rest` -> `rest`.
pub fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.trim_start_matches('_').chars() {
        if c == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A doc comment's text: the lines of its `#[doc = "..."]` attributes, each without the one space `///` leaves.
pub fn doc_text(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        if let syn::Meta::NameValue(meta) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) = &meta.value
        {
            for line in text.value().split('\n') {
                lines.push(
                    line.strip_prefix(' ')
                        .unwrap_or(line)
                        .trim_end()
                        .to_string(),
                );
            }
        }
    }
    lines.join("\n").trim().to_string()
}

/// The usage text's description from a doc comment: its first line that is not empty, trimmed.
pub fn first_line(docs: &str) -> &str {
    docs.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

/// What `#[function]` or a method's `#[torque(...)]` says.
#[derive(Default)]
struct Options {
    namespace: Option<LitStr>,
    name: Option<LitStr>,
    usage: Option<LitStr>,
    description: Option<LitStr>,
    dev: bool,
    skip: bool,
    /// The `Effects` constants `effects = "..."` names.
    effects: Vec<syn::Ident>,
}

/// What `#[methods]` says.
#[derive(Default)]
struct BlockOptions {
    class: Option<LitStr>,
    list: Option<syn::Ident>,
    krate: Option<syn::Path>,
}

fn expand_function(
    args: TokenStream,
    item: TokenStream,
    engine: &syn::Path,
) -> syn::Result<TokenStream> {
    let mut options = Options::default();
    let mut krate: Option<syn::Path> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("ns") {
            options.namespace = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("crate") {
            krate = Some(meta.value()?.parse()?);
        } else if !native_option(&meta, &mut options)? {
            return Err(meta.error(
                "expected `ns = \"...\"`, `name = \"...\"`, `usage = \"...\"`, `description = \"...\"`, `dev`, `effects = \"...\"` or `crate = path`",
            ));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    let engine = krate.as_ref().unwrap_or(engine);

    let mut f = match syn::parse2::<Item>(item)? {
        Item::Fn(f) => f,
        Item::Trait(t) => {
            return Err(syn::Error::new_spanned(
                t.trait_token,
                "`#[function]` goes on a free function; a trait's methods are declared with `#[methods]` on an impl \
                 of it for a concrete type",
            ));
        }
        Item::Impl(i) => {
            return Err(syn::Error::new_spanned(
                i.impl_token,
                "`#[function]` goes on a free function; an impl block's methods take `#[methods(class = \"...\")]`",
            ));
        }
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`#[function]` goes on a free function",
            ));
        }
    };
    if let Some(receiver) = f.sig.receiver() {
        return Err(syn::Error::new_spanned(
            receiver,
            "`self` in a free function: a native that is called on an object is a method, declared with \
             `#[methods(class = \"...\")]` on its impl block",
        ));
    }
    check_signature(&f.sig)?;
    let params = param_names(&mut f.sig)?;
    let types = param_types(&f.sig, None);

    let ident = f.sig.ident.clone();
    let vis = &f.vis;
    let namespace = options
        .namespace
        .as_ref()
        .map_or_else(String::new, LitStr::value);
    let name = script_name(&options, &ident);
    let docs = doc_text(&f.attrs);
    let description = description(&options, &docs);
    let decl = declaration(
        engine,
        &Declared {
            namespace: &namespace,
            name: &name,
            method: false,
            dev: options.dev,
            effects: &options.effects,
            params: &params,
            description: &description,
            usage: options.usage.as_ref().map(LitStr::value),
            docs: &docs,
        },
        &quote!(#engine::IntoNative::into_native(#ident)),
    );
    let check = shape_check(engine, &types, f.sig.ident.span());
    let doc = format!("The declaration of the native `{ident}`, for `natives!`.");
    Ok(quote! {
        #f

        #[doc = #doc]
        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        #vis struct #ident {}

        impl #engine::script::Natives for #ident {
            const NATIVES: &'static [#engine::script::NativeDecl] = &[#decl];
        }

        #check
    })
}

/// Reads one of the options a native takes (`name`, `usage`, `description`, `dev`) into `options`; false for another
/// key.
fn native_option(meta: &syn::meta::ParseNestedMeta, options: &mut Options) -> syn::Result<bool> {
    if meta.path.is_ident("name") {
        let name: LitStr = meta.value()?.parse()?;
        if !is_script_name(&name.value()) {
            return Err(syn::Error::new_spanned(
                &name,
                "a script name is ASCII letters, digits and '_', not starting with a digit",
            ));
        }
        options.name = Some(name);
    } else if meta.path.is_ident("usage") {
        options.usage = Some(meta.value()?.parse()?);
    } else if meta.path.is_ident("description") {
        options.description = Some(meta.value()?.parse()?);
    } else if meta.path.is_ident("dev") {
        options.dev = true;
    } else if meta.path.is_ident("effects") {
        let text: LitStr = meta.value()?.parse()?;
        options.effects = effect_names(&text)?;
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// The effects named in `text`, joined with `+`, as the `Effects` constants' idents.
fn effect_names(text: &LitStr) -> syn::Result<Vec<syn::Ident>> {
    text.value()
        .split('+')
        .map(|name| match name {
            "files" | "code" | "engine" | "network" | "permissions" | "variables" => {
                Ok(syn::Ident::new(&name.to_ascii_uppercase(), text.span()))
            }
            _ => Err(syn::Error::new_spanned(
                text,
                "the effects are `files`, `code`, `engine`, `network`, `permissions` and `variables`, joined with `+`",
            )),
        })
        .collect()
}

pub(crate) fn is_script_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| !b.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The name scripts call: the option's, else the Rust name in camelCase.
fn script_name(options: &Options, ident: &syn::Ident) -> String {
    options
        .name
        .as_ref()
        .map_or_else(|| camel_case(&ident.unraw().to_string()), LitStr::value)
}

fn description(options: &Options, docs: &str) -> String {
    options
        .description
        .as_ref()
        .map_or_else(|| first_line(docs).to_string(), LitStr::value)
}

/// What a signature must not be for a native: generic over types, async, or variadic.
fn check_signature(sig: &Signature) -> syn::Result<()> {
    if let Some(param) = sig
        .generics
        .params
        .iter()
        .find(|p| !matches!(p, syn::GenericParam::Lifetime(_)))
    {
        return Err(syn::Error::new_spanned(
            param,
            "a native is not generic: each parameter's type decides how it is read",
        ));
    }
    if let Some(token) = &sig.asyncness {
        return Err(syn::Error::new_spanned(
            token,
            "a native runs to its end in the call: it is not `async`",
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(syn::Error::new_spanned(
            variadic,
            "a native takes the rest of its arguments as `Rest`",
        ));
    }
    for input in &sig.inputs {
        if let FnArg::Typed(arg) = input
            && let Type::ImplTrait(ty) = &*arg.ty
        {
            return Err(syn::Error::new_spanned(
                ty,
                "a native's parameter has a concrete type: each type decides how the parameter is read",
            ));
        }
    }
    Ok(())
}

/// The names of the parameters (all but a receiver), from `#[arg(name = "...")]` or the pattern in camelCase
/// (`argN` for a pattern that is not a name), with the `#[arg]` attributes taken off.
fn param_names(sig: &mut Signature) -> syn::Result<Vec<String>> {
    let mut names = Vec::new();
    let typed = sig.inputs.iter_mut().filter_map(|input| match input {
        FnArg::Typed(arg) => Some(arg),
        FnArg::Receiver(_) => None,
    });
    for (i, arg) in typed.enumerate() {
        let mut given: Option<String> = None;
        let mut error: Option<syn::Error> = None;
        arg.attrs.retain(|attr| {
            if !attr.path().is_ident("arg") {
                return true;
            }
            let read = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let name: LitStr = meta.value()?.parse()?;
                    if name.value().is_empty() {
                        return Err(syn::Error::new_spanned(
                            &name,
                            "an argument's name is not empty",
                        ));
                    }
                    given = Some(name.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `name = \"...\"`"))
                }
            });
            if let Err(e) = read {
                error.get_or_insert(e);
            }
            false
        });
        if let Some(error) = error {
            return Err(error);
        }
        let name = given.unwrap_or_else(|| match &*arg.pat {
            Pat::Ident(pat) if !camel_case(&pat.ident.unraw().to_string()).is_empty() => {
                camel_case(&pat.ident.unraw().to_string())
            }
            _ => format!("arg{}", i + 1),
        });
        names.push(name);
    }
    Ok(names)
}

/// The parameters' types (all but a receiver) as they can be named in a constant: named lifetimes become `'_`, and
/// `Self` becomes `self_ty` when given.
fn param_types(sig: &Signature, self_ty: Option<&Type>) -> Vec<TokenStream> {
    sig.inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(arg) => Some(const_type(&arg.ty, self_ty)),
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// `ty` with its lifetimes as `'_` and `Self` as `self_ty`.
fn const_type(ty: &Type, self_ty: Option<&Type>) -> TokenStream {
    let mut ty = ty.clone();
    struct Erase;
    impl syn::visit_mut::VisitMut for Erase {
        fn visit_lifetime_mut(&mut self, lifetime: &mut syn::Lifetime) {
            if lifetime.ident != "static" {
                lifetime.ident = syn::Ident::new("_", lifetime.ident.span());
            }
        }
    }
    syn::visit_mut::visit_type_mut(&mut Erase, &mut ty);
    match self_ty {
        Some(self_ty) => replace_self(ty.into_token_stream(), self_ty),
        None => ty.into_token_stream(),
    }
}

/// `tokens` with each `Self` as `self_ty`: types named in a constant or a function outside the impl block.
fn replace_self(tokens: TokenStream, self_ty: &Type) -> TokenStream {
    tokens
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Ident(ident) if ident == "Self" => self_ty.to_token_stream(),
            TokenTree::Group(group) => {
                let mut out = proc_macro2::Group::new(
                    group.delimiter(),
                    replace_self(group.stream(), self_ty),
                );
                out.set_span(group.span());
                TokenTree::Group(out).into_token_stream()
            }
            other => other.into_token_stream(),
        })
        .collect()
}

/// The constant that refuses a signature the engine cannot run, at its `span`, when the crate is checked: the
/// parameters' kinds, checked by the engine's rule. A type that is neither a script argument nor injected fails here
/// too, at the type.
fn shape_check(engine: &syn::Path, types: &[TokenStream], span: Span) -> TokenStream {
    let kinds = types
        .iter()
        .map(|ty| quote_spanned!(ty.span()=> <#ty as #engine::Param>::KIND));
    quote_spanned! {span=>
        const _: () = match #engine::param::native_shape_error(&[#(#kinds),*]) {
            ::core::option::Option::Some(error) => ::core::panic!("{}", error),
            ::core::option::Option::None => {}
        };
    }
}

/// What a declaration holds besides the function that makes the native.
struct Declared<'a> {
    namespace: &'a str,
    name: &'a str,
    method: bool,
    dev: bool,
    effects: &'a [syn::Ident],
    params: &'a [String],
    description: &'a str,
    usage: Option<String>,
    docs: &'a str,
}

/// The `NativeDecl` constant, with `make` the expression that makes the `TypedNative`.
fn declaration(engine: &syn::Path, d: &Declared, make: &TokenStream) -> TokenStream {
    let Declared {
        namespace,
        name,
        method,
        dev,
        effects,
        params,
        description,
        docs,
        ..
    } = d;
    let usage = match &d.usage {
        Some(usage) => quote!(::core::option::Option::Some(#usage)),
        None => quote!(::core::option::Option::None),
    };
    quote! {
        #engine::script::NativeDecl {
            namespace: #namespace,
            name: #name,
            method: #method,
            dev: #dev,
            params: &[#(#params),*],
            description: #description,
            usage: #usage,
            docs: #docs,
            effects: #engine::Effects::NONE #(.union(#engine::Effects::#effects))*,
            native: {
                fn __torque_native() -> #engine::script::TypedNative {
                    #make
                }
                __torque_native
            },
        }
    }
}

fn expand_methods(
    args: TokenStream,
    item: TokenStream,
    engine: &syn::Path,
) -> syn::Result<TokenStream> {
    let mut options = BlockOptions::default();
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("class") {
            let class: LitStr = meta.value()?.parse()?;
            if !is_script_name(&class.value()) {
                return Err(syn::Error::new_spanned(
                    &class,
                    "a class name is ASCII letters, digits and '_', not starting with a digit",
                ));
            }
            options.class = Some(class);
        } else if meta.path.is_ident("list") {
            options.list = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("crate") {
            options.krate = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("expected `class = \"...\"`, `list = Name` or `crate = path`"));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    let engine = options.krate.as_ref().unwrap_or(engine);

    let mut block = match syn::parse2::<Item>(item)? {
        Item::Impl(block) => block,
        Item::Trait(t) => {
            return Err(syn::Error::new_spanned(
                t.trait_token,
                "`#[methods]` goes on an impl block for a concrete type, not on a trait definition: put it on \
                 `impl Trait for Type`",
            ));
        }
        Item::Fn(f) => {
            return Err(syn::Error::new_spanned(
                f.sig.fn_token,
                "`#[methods]` goes on an impl block; a free function takes `#[function]`",
            ));
        }
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`#[methods]` goes on an impl block",
            ));
        }
    };
    if !block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &block.generics,
            "`#[methods]` goes on an impl block for a concrete type: each generic instance would declare the same \
             natives",
        ));
    }
    let self_ty = (*block.self_ty).clone();
    let Type::Path(self_path) = &self_ty else {
        return Err(syn::Error::new_spanned(
            &self_ty,
            "`#[methods]` goes on an impl block for a named type",
        ));
    };
    let class = match &options.class {
        Some(class) => class.value(),
        None => self_path
            .path
            .segments
            .last()
            .map(|s| s.ident.unraw().to_string())
            .unwrap_or_default(),
    };
    let in_trait = block.trait_.as_ref().map(|(path, _)| path.clone());
    let qualified = match &in_trait {
        Some(trait_path) => quote!(<#self_ty as #trait_path>),
        None => quote!(<#self_ty>),
    };

    let mut decls = Vec::new();
    let mut checks = Vec::new();
    // The names taken: lower-cased, and as written.
    let mut taken: Vec<(String, String)> = Vec::new();
    for item in &mut block.items {
        let ImplItem::Fn(f) = item else {
            continue;
        };
        let options = take_torque_options(&mut f.attrs)?;
        let selected = in_trait.is_some() || matches!(f.vis, Visibility::Public(_));
        if !selected {
            if let Some((_, span)) = options {
                return Err(syn::Error::new(
                    span,
                    "only `pub` fns of an inherent block become methods: make it `pub`, or drop the attribute",
                ));
            }
            continue;
        }
        let options = options.map(|(o, _)| o).unwrap_or_default();
        if options.skip {
            strip_arg_attrs(&mut f.sig);
            continue;
        }
        check_signature(&f.sig)?;
        let name = script_name(&options, &f.sig.ident);
        let key = name.to_ascii_lowercase();
        if let Some((_, first)) = taken.iter().find(|(k, _)| *k == key) {
            return Err(syn::Error::new_spanned(
                &f.sig.ident,
                format!(
                    "two methods of this block are called `{name}` (`{first}` came first; names compare without \
                     case): rename one with `#[torque(name = \"...\")]` or leave one out with `#[torque(skip)]`"
                ),
            ));
        }
        taken.push((key, name.clone()));
        let docs = doc_text(&f.attrs);
        let description = description(&options, &docs);
        let mut params = param_names(&mut f.sig)?;
        let types = param_types(&f.sig, Some(&self_ty));
        checks.push(shape_check(engine, &types, f.sig.ident.span()));
        let ident = &f.sig.ident;
        let make = match f.sig.receiver() {
            None => quote!(#engine::IntoNative::into_native(#qualified::#ident)),
            Some(receiver) => {
                if !matches!(receiver.kind, syn::ReceiverKind::Reference(..)) {
                    return Err(syn::Error::new_spanned(
                        receiver,
                        "a method is called on its object's data as `&self` or `&mut self`, or takes `This` for \
                         the object",
                    ));
                }
                let (make, names) = receiver_wrapper(engine, &f.sig, &self_ty, &qualified, &params);
                params = names;
                make
            }
        };
        decls.push(declaration(
            engine,
            &Declared {
                namespace: &class,
                name: &name,
                method: true,
                dev: false,
                effects: &options.effects,
                params: &params,
                description: &description,
                usage: options.usage.as_ref().map(LitStr::value),
                docs: &docs,
            },
            &make,
        ));
    }

    let (list_item, list_ty) = match &options.list {
        Some(list) => {
            let doc = format!(
                "The methods of `{class}` its `#[methods]` block declared, for `natives!`."
            );
            (
                quote! {
                    #[doc = #doc]
                    #[allow(dead_code)]
                    pub struct #list {}
                },
                list.to_token_stream(),
            )
        }
        None => (TokenStream::new(), self_ty.to_token_stream()),
    };
    Ok(quote! {
        #block

        #list_item

        impl #engine::script::Natives for #list_ty {
            const NATIVES: &'static [#engine::script::NativeDecl] = &[#(#decls),*];
        }

        #(#checks)*
    })
}

/// Takes a fn's `#[torque(...)]` attributes off and reads them (with the first one's span); None without any.
fn take_torque_options(attrs: &mut Vec<Attribute>) -> syn::Result<Option<(Options, Span)>> {
    let mut found: Option<(Options, Span)> = None;
    let mut error: Option<syn::Error> = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("torque") {
            return true;
        }
        let (options, _) = found.get_or_insert_with(|| (Options::default(), attr.path().span()));
        let read = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                options.skip = true;
                Ok(())
            } else if meta.path.is_ident("dev") {
                Err(meta.error("a method is not a development command: only functions are"))
            } else if native_option(&meta, options)? {
                Ok(())
            } else {
                Err(meta.error(
                    "expected `skip`, `name = \"...\"`, `usage = \"...\"`, `description = \"...\"` or `effects = \"...\"`",
                ))
            }
        });
        if let Err(e) = read {
            error.get_or_insert(e);
        }
        false
    });
    match error {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// Takes the `#[arg(...)]` attributes off a skipped fn's parameters.
fn strip_arg_attrs(sig: &mut Signature) {
    for input in &mut sig.inputs {
        if let FnArg::Typed(arg) = input {
            arg.attrs.retain(|attr| !attr.path().is_ident("arg"));
        }
    }
}

/// Whether `ty` names the parameter `name` (`VmMut` or `VmRef`) by its last path segment.
fn is_param_named(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name),
        Type::Group(group) => is_param_named(&group.elem, name),
        _ => false,
    }
}

/// The native of a method with `&self` or `&mut self`: a function that takes the object (`This`), the script engine
/// and the method's other parameters but its `VmMut`/`VmRef`, and calls the method on the object's data with the script
/// engine handed on; and the names of that function's parameters.
fn receiver_wrapper(
    engine: &syn::Path,
    sig: &Signature,
    self_ty: &Type,
    qualified: &TokenStream,
    names: &[String],
) -> (TokenStream, Vec<String>) {
    let method = &sig.ident;
    let lifetimes: Vec<_> = sig.generics.lifetimes().collect();
    let mut wrapper_params = Vec::new();
    let mut wrapper_names = vec!["this".to_string(), "vm".to_string()];
    let mut call_args = Vec::new();
    let typed = sig.inputs.iter().filter_map(|input| match input {
        FnArg::Typed(arg) => Some(arg),
        FnArg::Receiver(_) => None,
    });
    for (i, (arg, name)) in typed.zip(names).enumerate() {
        let value = format_ident!("__torque_arg{i}");
        if is_param_named(&arg.ty, "VmMut") {
            call_args.push(quote!(#engine::VmMut::new(__torque_vm)));
        } else if is_param_named(&arg.ty, "VmRef") {
            call_args.push(quote!(#engine::VmRef::new(&*__torque_vm)));
        } else {
            let ty = replace_self(arg.ty.to_token_stream(), self_ty);
            wrapper_params.push(quote!(#value: #ty));
            wrapper_names.push(name.clone());
            call_args.push(quote!(#value));
        }
    }
    let make = quote! {
        #[allow(unused_variables)]
        fn __torque_call<#(#lifetimes),*>(
            __torque_this: #engine::This,
            mut __torque_vm: #engine::VmMut<'_>,
            #(#wrapper_params),*
        ) -> #engine::script::MethodResult {
            #engine::script::with_object_data(
                &mut *__torque_vm,
                __torque_this,
                |__torque_data: &mut #self_ty, __torque_vm| #qualified::#method(__torque_data, #(#call_args),*),
            )
        }
        #engine::IntoNative::into_native(__torque_call)
    };
    (make, wrapper_names)
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
