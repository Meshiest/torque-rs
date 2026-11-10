//! Console functions, methods and globals an extension declares: typed natives ([`IntoNative`]) whose argument counts
//! and usage text come from their signature, as a compiled plugin's do, and the declarations `#[function]` makes.

use crate::param::{ArgKind, Fetch, IntoReturn, Param, ParamKind, Return, native_shape_error};
use crate::{Call, Side};

/// A typed native: a function whose parameters are script arguments and injected [`Param`]s, made by [`IntoNative`].
pub struct TypedNative {
    pub(crate) kinds: Vec<ParamKind>,
    pub(crate) run: Box<dyn Fn(Call<'_>) -> Return + Send + Sync>,
}

impl TypedNative {
    /// The kinds of its script arguments, in order.
    pub fn args(&self) -> Vec<ArgKind> {
        self.kinds
            .iter()
            .filter_map(|kind| match kind {
                ParamKind::Script(kind) => Some(*kind),
                ParamKind::Injected => None,
            })
            .collect()
    }

    /// The kinds of all its parameters, in order.
    pub fn params(&self) -> &[ParamKind] {
        &self.kinds
    }
}

/// A function an extension takes as a native: up to 12 parameters, each a script argument or an injected [`Param`],
/// returning an [`IntoReturn`] value. It is `Send + Sync`: the engine may call it from any of its threads (one call into
/// the extension at a time). `M` tells the parameter lists apart; it is inferred.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a native: each parameter must be a script argument or `Call` or `This`",
    label = "a parameter of this function is neither"
)]
pub trait IntoNative<M> {
    /// The native.
    fn into_native(self) -> TypedNative;
}

macro_rules! native_fns {
    ($($P:ident),*) => {
        impl<F, R, $($P,)*> IntoNative<fn($($P,)*) -> R> for F
        where
            F: Fn($($P),*) -> R + Fn($(<$P as Param>::Item<'_>),*) -> R + Send + Sync + 'static,
            R: IntoReturn + 'static,
            $($P: Param + 'static,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn into_native(self) -> TypedNative {
                const {
                    if let Some(error) = native_shape_error(&[$(<$P as Param>::KIND),*]) {
                        panic!("{}", error);
                    }
                }
                let kinds: &[ParamKind] = &[$(<$P as Param>::KIND),*];
                TypedNative {
                    kinds: kinds.to_vec(),
                    run: Box::new(move |call: Call<'_>| -> Return {
                        let cx = Fetch { call, next: std::cell::Cell::new(0), count: call.arg_count() };
                        $(let $P = match <$P as Param>::fetch(&cx) {
                            Ok(param) => param,
                            Err(error) => return Return::Error(error),
                        };)*
                        (self)($($P),*).into_return()
                    }),
                }
            }
        }
    };
}

native_fns!();
native_fns!(T1);
native_fns!(T1, T2);
native_fns!(T1, T2, T3);
native_fns!(T1, T2, T3, T4);
native_fns!(T1, T2, T3, T4, T5);
native_fns!(T1, T2, T3, T4, T5, T6);
native_fns!(T1, T2, T3, T4, T5, T6, T7);
native_fns!(T1, T2, T3, T4, T5, T6, T7, T8);
native_fns!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
native_fns!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
native_fns!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
native_fns!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);

/// A native the `#[function]` attribute declared, as a constant (the same fields the engine's declarations have).
/// [`natives!`](crate::natives) lists them for [`ScriptRegistry::natives`].
#[derive(Clone, Copy, Debug)]
pub struct NativeDecl {
    /// A function's namespace (empty for the global one), or a method's class.
    pub namespace: &'static str,
    /// The name scripts call it by.
    pub name: &'static str,
    /// A method, not a function.
    pub method: bool,
    /// A development command.
    pub dev: bool,
    /// A name for each parameter, in order, injected ones too: the script arguments' names are the usage text's.
    pub params: &'static [&'static str],
    /// What follows the signature in the usage text; empty for nothing.
    pub description: &'static str,
    /// The usage text exactly as written, in place of the generated one.
    pub usage: Option<&'static str>,
    /// The whole doc comment, for the script reference.
    pub docs: &'static str,
    /// What it does beyond its result (the attribute's `effects = "files+code"`): see [`ScriptRegistry::with_effects`].
    pub effects: Effects,
    /// Makes the native.
    pub native: fn() -> TypedNative,
}

/// A list of declared natives: what [`natives!`](crate::natives) takes. The `#[function]` attribute implements it for
/// a hidden type named as its function.
pub trait Natives {
    /// The natives.
    const NATIVES: &'static [NativeDecl];
}

/// Whether two natives of `lists` take one name (namespace and name, without case).
#[doc(hidden)]
pub const fn has_duplicate(lists: &[&[NativeDecl]]) -> bool {
    let mut a = 0;
    while a < lists.len() {
        let mut i = 0;
        while i < lists[a].len() {
            let mut b = a;
            while b < lists.len() {
                let mut j = if b == a { i + 1 } else { 0 };
                while j < lists[b].len() {
                    let (x, y) = (&lists[a][i], &lists[b][j]);
                    if same_name(x.namespace, y.namespace) && same_name(x.name, y.name) {
                        return true;
                    }
                    j += 1;
                }
                b += 1;
            }
            i += 1;
        }
        a += 1;
    }
    false
}

/// Whether two names are one without ASCII case.
const fn same_name(a: &str, b: &str) -> bool {
    a.as_bytes().eq_ignore_ascii_case(b.as_bytes())
}

/// Lists natives the `#[function]` attribute declared, for [`ScriptRegistry::natives`]: `natives![greet, wave]`. Two
/// of one name do not compile.
#[macro_export]
macro_rules! natives {
    ($($native:path),* $(,)?) => {{
        const _: () = ::core::assert!(
            !$crate::script::has_duplicate(&[$(<$native as $crate::script::Natives>::NATIVES),*]),
            "two natives of this list have the same name (names compare without case): rename one with `name = \"...\"`"
        );
        #[allow(unused_mut)]
        let mut natives: ::std::vec::Vec<$crate::script::NativeDecl> = ::std::vec::Vec::new();
        $(natives.extend_from_slice(<$native as $crate::script::Natives>::NATIVES);)*
        natives
    }};
}

/// One native as it goes to the engine.
pub(crate) struct NativeReg {
    pub(crate) side: Side,
    pub(crate) method: bool,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) args: Vec<(String, ArgKind)>,
    pub(crate) description: String,
    pub(crate) usage: Option<String>,
    pub(crate) docs: String,
    pub(crate) dev: bool,
    pub(crate) effects: Effects,
    pub(crate) native: TypedNative,
}

/// What a native does beyond its result, declared with it ([`ScriptRegistry::with_effects`], or the `#[function]`
/// attribute's `effects = "files+code"`): the engine's default call policy keeps script languages away from natives with
/// [`FILES`](Self::FILES), [`CODE`](Self::CODE) or [`ENGINE`](Self::ENGINE).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Effects(u32);

impl Effects {
    /// No effect beyond its result and the objects and variables it changes.
    pub const NONE: Effects = Effects(0);
    /// It opens, writes, deletes or exports files.
    pub const FILES: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_FILES);
    /// It runs code from text, or a function or method it names.
    pub const CODE: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_CODE);
    /// It switches or loads mods, deletes every datablock or variable, or quits.
    pub const ENGINE: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_ENGINE);
    /// It connects to, or sends to, an address its caller names.
    pub const NETWORK: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_NETWORK);
    /// It grants, revokes or defines permissions or roles.
    pub const PERMISSIONS: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_PERMISSIONS);
    /// It writes a variable whose name one of its arguments gives.
    pub const VARIABLES: Effects = Effects(torque_plugin_abi::TORQUE_NATIVE_VARIABLES);

    /// Both sets.
    #[must_use]
    pub const fn union(self, other: Effects) -> Effects {
        Effects(self.0 | other.0)
    }

    /// The declaration's flags for them.
    pub(crate) const fn flags(self) -> u32 {
        self.0
    }
}

impl std::ops::BitOr for Effects {
    type Output = Effects;

    fn bitor(self, other: Effects) -> Effects {
        self.union(other)
    }
}

/// The console functions, methods and globals an extension declares, from [`Registrar::script`](crate::Registrar::script)
/// (every script engine), [`server_script`](crate::Registrar::server_script) or
/// [`client_script`](crate::Registrar::client_script). The engine registers each into every script engine of its side,
/// as it does a compiled plugin's.
pub struct ScriptRegistry<'r, 'a> {
    pub(crate) registrar: &'r mut crate::Registrar<'a>,
    pub(crate) side: Side,
    pub(crate) effects: Effects,
}

impl ScriptRegistry<'_, '_> {
    /// Declares what `declare` declares in this registry with `effects` (on top of what each declaration says itself): a
    /// native that writes or deletes files, runs code from text, switches mods or quits, changes permissions, or connects to
    /// an address it is given must say so, so the engine's call policy can keep script languages away from it. Only what
    /// `declare` declares gets them; the registry is as it was before once it returns.
    pub fn with_effects(&mut self, effects: Effects, declare: impl FnOnce(&mut Self)) -> &mut Self {
        let kept = self.effects;
        self.effects = kept | effects;
        declare(self);
        self.effects = kept;
        self
    }

    /// Declares the function `namespace::name` (`namespace` empty for a global function): `f`, with `args` naming its
    /// script arguments and `description` after the signature in its usage text.
    pub fn function<M>(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[&str],
        description: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        self.typed(
            false,
            false,
            namespace,
            name,
            args,
            description,
            f.into_native(),
        )
    }

    /// Declares the method `name` of `class` (`%obj.name(..)`): `f` gets the object as [`This`](crate::This).
    pub fn method<M>(
        &mut self,
        class: &str,
        name: &str,
        args: &[&str],
        description: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        self.typed(true, false, class, name, args, description, f.into_native())
    }

    /// Declares a development command: a [`function`](Self::function) that runs in a debug build of the engine, or once
    /// `$Pref::Dev` is true.
    pub fn dev<M>(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[&str],
        description: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        self.typed(
            false,
            true,
            namespace,
            name,
            args,
            description,
            f.into_native(),
        )
    }

    /// Declares the natives the `#[function]` attribute made, as [`natives!`](crate::natives) lists them.
    pub fn natives(&mut self, natives: impl IntoIterator<Item = NativeDecl>) -> &mut Self {
        for decl in natives {
            let native = (decl.native)();
            if decl.params.len() != native.kinds.len() {
                self.registrar.refuse(format!(
                    "`{}` names {} parameter(s), but its function takes {}",
                    decl.name,
                    decl.params.len(),
                    native.kinds.len()
                ));
                continue;
            }
            let args = decl
                .params
                .iter()
                .zip(&native.kinds)
                .filter_map(|(name, kind)| match kind {
                    ParamKind::Script(kind) => Some((name.to_string(), *kind)),
                    ParamKind::Injected => None,
                })
                .collect();
            self.registrar.native(NativeReg {
                side: self.side,
                method: decl.method,
                namespace: decl.namespace.to_string(),
                name: decl.name.to_string(),
                args,
                description: decl.description.to_string(),
                usage: decl.usage.map(str::to_string),
                docs: decl.docs.to_string(),
                dev: decl.dev,
                effects: self.effects | decl.effects,
                native,
            });
        }
        self
    }

    /// Declares the global variable `name` (with its `$`) and sets it to `default`, before scripts run.
    pub fn global(&mut self, name: &str, default: &str) -> &mut Self {
        self.registrar.global(self.side, name, default);
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn typed(
        &mut self,
        method: bool,
        dev: bool,
        namespace: &str,
        name: &str,
        args: &[&str],
        description: &str,
        native: TypedNative,
    ) -> &mut Self {
        let kinds = native.args();
        if kinds.len() != args.len() {
            self.registrar.refuse(format!(
                "`{name}` names {} script argument(s), but its function takes {}",
                args.len(),
                kinds.len()
            ));
            return self;
        }
        self.registrar.native(NativeReg {
            side: self.side,
            method,
            namespace: namespace.to_string(),
            name: name.to_string(),
            args: args.iter().map(|a| a.to_string()).zip(kinds).collect(),
            description: description.to_string(),
            usage: None,
            docs: String::new(),
            dev,
            effects: self.effects,
            native,
        });
        self
    }
}
