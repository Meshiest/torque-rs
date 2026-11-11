//! The script registry: console functions, methods and globals that plugins declare, registered into every script engine
//! the engine makes, and typed natives ([`IntoNative`]) whose argument counts and usage text come from their signature.

use std::any::{Any, type_name};
use std::collections::BTreeMap;
use std::rc::Rc;

use torque_ts_vm::convert::{ArgKind, ScriptError, ScriptReturn};
use torque_ts_vm::effects::Effects;
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use crate::param::{Cx, Param, ParamError, ParamKind, all_arities, native_shape_error};
use crate::{BuildError, ClientStage, Engine, EngineBuilder, PluginId, ServerStage, Stage, This};

/// The global that lets a release build run [development commands](ScriptRegistry::dev).
pub const DEV_PREF: &str = "$Pref::Dev";

/// The server stage at which the registry's entries are registered: after its recipes, the last before the server's
/// scripts run.
pub const SERVER_SCRIPT_STAGE: ServerStage = ServerStage::Net;

/// The client stage at which the registry's entries are registered: after its recipes, the last before the client's
/// scripts run.
pub const CLIENT_SCRIPT_STAGE: ClientStage = ClientStage::Natives;

/// Which script engines a registry entry goes into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// Every server's.
    Server,
    /// Every client's.
    Client,
    /// Both.
    Both,
}

impl Side {
    fn server(self) -> bool {
        matches!(self, Side::Server | Side::Both)
    }

    fn client(self) -> bool {
        matches!(self, Side::Client | Side::Both)
    }

    fn overlaps(self, other: Side) -> bool {
        (self.server() && other.server()) || (self.client() && other.client())
    }
}

/// What a native is called as: a function (in the global namespace, or one such as `Pad`) or a method of a class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Callee<'a> {
    /// `name(..)`, or `Namespace::name(..)`.
    Function {
        /// The namespace: empty for the global one.
        namespace: &'a str,
        /// The name.
        name: &'a str,
    },
    /// `%object.name(..)`: its `argv[1]` is the object.
    Method {
        /// The class (its namespace).
        class: &'a str,
        /// The name.
        name: &'a str,
    },
}

impl Callee<'_> {
    /// The namespace it is registered in.
    pub fn namespace(&self) -> &str {
        match self {
            Callee::Function { namespace, .. } => namespace,
            Callee::Method { class, .. } => class,
        }
    }

    /// Its name.
    pub fn name(&self) -> &str {
        match self {
            Callee::Function { name, .. } | Callee::Method { name, .. } => name,
        }
    }

    /// The arguments before the script's own: the name, and for a method the object.
    fn fixed_args(&self) -> i32 {
        match self {
            Callee::Function { .. } => 1,
            Callee::Method { .. } => 2,
        }
    }

    /// `name`, `Namespace::name` or `Class::name`: how errors name it.
    pub fn label(&self) -> String {
        match self.namespace() {
            "" => self.name().to_string(),
            namespace => format!("{namespace}::{}", self.name()),
        }
    }
}

/// The usage text of a native: its signature made from the script arguments' `names` and `kinds`, then ` - ` and the
/// `description` (left off when empty).
///
/// A function reads `getWords(text, first [, last])`, one in a namespace `Pad::setDeadZone(zone)`, a method
/// `player.giveAmmo(amount [, max])` (the class with its first letter in lower case), the rest `args...`: the form of the
/// engine's own usage texts.
///
/// ```
/// use torque_engine::script::{Callee, usage_text};
/// use torque_ts_vm::convert::ArgKind::{Optional, Required, Rest};
///
/// let callee = Callee::Function { namespace: "", name: "getWords" };
/// let usage = usage_text(callee, &["text", "first", "last"], &[Required, Required, Optional], "the words from first to last");
/// assert_eq!(usage, "getWords(text, first [, last]) - the words from first to last");
///
/// let callee = Callee::Method { class: "SimObject", name: "schedule" };
/// assert_eq!(usage_text(callee, &["delay", "method", "args"], &[Required, Required, Rest], ""), "simObject.schedule(delay, method, args...)");
/// ```
pub fn usage_text(callee: Callee, names: &[&str], kinds: &[ArgKind], description: &str) -> String {
    let mut out = match callee {
        Callee::Function {
            namespace: "",
            name,
        } => name.to_string(),
        Callee::Function { namespace, name } => format!("{namespace}::{name}"),
        Callee::Method { class, name } => {
            let mut chars = class.chars();
            let first: String = chars
                .next()
                .into_iter()
                .flat_map(char::to_lowercase)
                .collect();
            format!("{first}{}.{name}", chars.as_str())
        }
    };
    out.push('(');
    let mut open = 0;
    for (i, (name, kind)) in names.iter().zip(kinds).enumerate() {
        let optional = *kind == ArgKind::Optional || (*kind == ArgKind::Rest && open > 0);
        match (optional, i) {
            (true, 0) => out.push('['),
            (true, _) => out.push_str(" [, "),
            (false, 0) => {}
            (false, _) => out.push_str(", "),
        }
        if optional {
            open += 1;
        }
        out.push_str(name);
        if *kind == ArgKind::Rest {
            out.push_str("...");
        }
    }
    out.extend(std::iter::repeat_n(']', open));
    out.push(')');
    if !description.is_empty() {
        out.push_str(" - ");
        out.push_str(description);
    }
    out
}

/// What a registered native needs at a call besides its function.
struct NativeInfo {
    label: String,
    usage: Rc<str>,
    first_arg: usize,
    dev: bool,
}

type MakeNative = dyn Fn(&Engine, Rc<NativeInfo>) -> Box<dyn Fn(&mut Vm, &NativeCall) -> Native>;

/// A typed native: a function whose parameters are script arguments ([`ScriptArg`](torque_ts_vm::convert::ScriptArg)
/// types) and injected [`Param`]s, made by [`IntoNative`]. Each script engine it is registered in gets its own state
/// ([`Local`](crate::Local)).
pub struct TypedNative {
    /// Every parameter's kind, injected ones too.
    kinds: Vec<ParamKind>,
    args: Vec<ArgKind>,
    make: Rc<MakeNative>,
}

impl TypedNative {
    /// The kinds of its script arguments, in order.
    pub fn args(&self) -> &[ArgKind] {
        &self.args
    }

    /// The kinds of all its parameters, in order: script arguments and injected ones.
    pub fn params(&self) -> &[ParamKind] {
        &self.kinds
    }

    /// The names of its script arguments out of `params`, a name for each of its parameters (injected ones too); Err
    /// when `params` does not name each parameter once.
    fn script_names<'n>(&self, callee: Callee, params: &[&'n str]) -> Result<Vec<&'n str>, String> {
        if params.len() != self.kinds.len() {
            return Err(format!(
                "`{}` names {} parameter(s), but its function takes {}",
                callee.label(),
                params.len(),
                self.kinds.len()
            ));
        }
        Ok(params
            .iter()
            .zip(&self.kinds)
            .filter(|(_, kind)| matches!(kind, ParamKind::Script(_)))
            .map(|(name, _)| *name)
            .collect())
    }

    /// Its argument counts for `callee`, as [`Vm::register_in`] takes them (the name, and a method's object, counted;
    /// `0` for no limit).
    pub fn counts(&self, callee: Callee) -> (i32, i32) {
        let fixed = callee.fixed_args();
        let required = self
            .args
            .iter()
            .filter(|k| **k == ArgKind::Required)
            .count() as i32;
        let optional = self
            .args
            .iter()
            .filter(|k| **k == ArgKind::Optional)
            .count() as i32;
        let rest = self.args.contains(&ArgKind::Rest);
        let min = fixed + required;
        (min, if rest { 0 } else { min + optional })
    }

    /// Registers the native in `vm` as `callee`, with `names` for its script arguments and `description` for its usage
    /// text ([`usage_text`]); `engine` is what its injected parameters come from. Err when `names` does not name each
    /// script argument once.
    pub fn register(
        &self,
        vm: &mut Vm,
        engine: &Engine,
        callee: Callee,
        names: &[&str],
        description: &str,
    ) -> Result<(), String> {
        let usage = self.usage(callee, names, description)?;
        self.install(vm, engine, callee, &usage, false);
        Ok(())
    }

    /// A native over the raw call, for one whose script arguments are known only at run time (a loaded extension's):
    /// `args` are their kinds, and `f` gets the script engine, the call and the index of its first script argument in
    /// `call.argv` (after the name, and a method's object). Its result and errors are a typed native's: an `Err` is
    /// printed with the native's name and usage, and the call returns the empty string. Err when `args` are not
    /// required first, then optional ones, then one rest last.
    ///
    /// ```
    /// use torque_engine::script::{Callee, TypedNative};
    /// use torque_engine::EngineBuilder;
    /// use torque_ts_vm::convert::ArgKind;
    /// use torque_ts_vm::vm::{Native, Vm};
    ///
    /// let native = TypedNative::from_call(&[ArgKind::Required], |_vm, call, first| {
    ///     Ok(Native::Str(call.argv[first].repeat(2)))
    /// })?;
    /// let engine = EngineBuilder::new().build()?;
    /// let mut vm = Vm::new();
    /// native.register(&mut vm, &engine, Callee::Function { namespace: "", name: "twice" }, &["text"], "")?;
    /// assert_eq!(vm.eval(b"return twice(\"ab\");")?, b"abab");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_call(
        args: &[ArgKind],
        f: impl Fn(&mut Vm, &NativeCall, usize) -> Result<Native, ScriptError> + 'static,
    ) -> Result<TypedNative, String> {
        let kinds: Vec<ParamKind> = args.iter().map(|&k| ParamKind::Script(k)).collect();
        if let Some(error) = native_shape_error(&kinds) {
            return Err(error.to_string());
        }
        let f = Rc::new(f);
        Ok(TypedNative {
            kinds,
            args: args.to_vec(),
            make: Rc::new(move |_engine: &Engine, info: Rc<NativeInfo>| {
                let f = Rc::clone(&f);
                Box::new(move |vm: &mut Vm, call: &NativeCall| -> Native {
                    if !dev_gate(vm, &info) {
                        return Native::Str(Vec::new());
                    }
                    let outcome = f(vm, call, info.first_arg);
                    finish(vm, &info, Ok(outcome))
                })
            }),
        })
    }

    fn usage(&self, callee: Callee, names: &[&str], description: &str) -> Result<String, String> {
        if names.len() != self.args.len() {
            return Err(format!(
                "`{}` names {} script argument(s), but its function takes {}",
                callee.label(),
                names.len(),
                self.args.len()
            ));
        }
        Ok(usage_text(callee, names, &self.args, description))
    }

    fn install(&self, vm: &mut Vm, engine: &Engine, callee: Callee, usage: &str, dev: bool) {
        let (min, max) = self.counts(callee);
        let info = Rc::new(NativeInfo {
            label: callee.label(),
            usage: Rc::from(usage),
            first_arg: callee.fixed_args() as usize,
            dev,
        });
        let f = (self.make)(engine, info);
        vm.register_in(callee.namespace(), callee.name(), min, max, usage, f);
    }
}

/// A function the [`ScriptRegistry`] takes as a native: up to 12 parameters, each a script argument
/// ([`ScriptArg`](torque_ts_vm::convert::ScriptArg): a number, a truth value, text, `Option<T>` for one that may be left
/// out, [`Rest`](torque_ts_vm::convert::Rest) for the rest) or an injected [`Param`], returning a
/// [`ScriptReturn`] (an [`IntoScript`](torque_ts_vm::convert::IntoScript) value or `Result<T, ScriptError>`).
///
/// The script arguments set the argument counts: optional ones only after the required ones, the rest last. At most one
/// [`VmMut`](crate::VmMut), and none beside a [`VmRef`](crate::VmRef). A signature that breaks these does not compile.
///
/// `M` tells the parameter lists apart; it is inferred.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a native: each parameter must be a script argument or an injected `Param`",
    label = "a parameter of this function is neither"
)]
pub trait IntoNative<M> {
    /// The native.
    fn into_native(self) -> TypedNative;
}

/// Whether a development command may run: always in a debug build, else when [`DEV_PREF`] is true.
fn dev_allowed(vm: &Vm) -> bool {
    dev_allowed_in(vm, cfg!(debug_assertions))
}

/// [`dev_allowed`] in a debug build or not.
pub(crate) fn dev_allowed_in(vm: &Vm, debug_build: bool) -> bool {
    debug_build
        || vm
            .global(DEV_PREF)
            .is_some_and(|value| torque_ts_vm::fields::atob(&value))
}

/// Ends a typed native's call: its result, or the error printed (with the usage for its own error) and the empty
/// string.
fn finish(
    vm: &mut Vm,
    info: &NativeInfo,
    outcome: Result<Result<Native, ScriptError>, ParamError>,
) -> Native {
    match outcome {
        Ok(Ok(native)) => return native,
        Ok(Err(error)) => {
            vm.error(format!("{}: {error}", info.label));
            vm.warn(format!("{}: usage: {}", info.label, info.usage));
        }
        Err(error) => vm.error(format!("{}: {error}", info.label)),
    }
    Native::Str(Vec::new())
}

/// The check a development command makes first: false (and the error printed) when it may not run.
fn dev_gate(vm: &mut Vm, info: &NativeInfo) -> bool {
    if !info.dev || dev_allowed(vm) {
        return true;
    }
    vm.error(format!(
        "{}: a development command: set {DEV_PREF} to 1 to use it",
        info.label
    ));
    false
}

macro_rules! native_fns {
    ($($P:ident),*) => {
        impl<F, R, $($P,)*> IntoNative<fn($($P,)*) -> R> for F
        where
            F: Fn($($P),*) -> R + Fn($(<$P as Param>::Item<'_>),*) -> R + 'static,
            R: ScriptReturn + 'static,
            $($P: Param + 'static,)*
        {
            // The parameters are named after their types; with none, nothing is fetched.
            #[allow(non_snake_case, unused_variables, unused_labels)]
            fn into_native(self) -> TypedNative {
                const {
                    if let Some(error) = native_shape_error(&[$(<$P as Param>::KIND),*]) {
                        panic!("{}", error);
                    }
                }
                let kinds: &[ParamKind] = &[$(<$P as Param>::KIND),*];
                let args = kinds
                    .iter()
                    .filter_map(|kind| match kind {
                        ParamKind::Script(kind) => Some(*kind),
                        _ => None,
                    })
                    .collect();
                let f = Rc::new(self);
                TypedNative {
                    kinds: kinds.to_vec(),
                    args,
                    make: Rc::new(move |engine: &Engine, info: Rc<NativeInfo>| {
                        let f = Rc::clone(&f);
                        let engine = engine.clone();
                        let state: ($(<$P as Param>::State,)*) = Default::default();
                        Box::new(move |vm: &mut Vm, call: &NativeCall| -> Native {
                            if !dev_gate(vm, &info) {
                                return Native::Str(Vec::new());
                            }
                            let outcome: Result<Result<Native, ScriptError>, ParamError> = 'call: {
                                let cx = Cx::native(&mut *vm, &engine, call, info.first_arg);
                                let ($($P,)*) = &state;
                                $(let $P = match <$P as Param>::fetch(&cx, $P) {
                                    Ok(param) => param,
                                    Err(error) => break 'call Err(error),
                                };)*
                                Ok((f)($($P),*).into_result())
                            };
                            finish(vm, &info, outcome)
                        })
                    }),
                }
            }
        }
    };
}

all_arities!(native_fns);

/// One thing the registry declared.
pub(crate) struct ScriptEntry {
    pub(crate) side: Side,
    pub(crate) kind: EntryKind,
}

/// A native the registry declared, as a reference page lists it ([`Engine::natives`]).
#[derive(Clone, Copy, Debug)]
pub struct NativeEntry<'a> {
    /// Which script engines it goes into.
    pub side: Side,
    /// A function's namespace (empty for the global one), or a method's class.
    pub namespace: &'a str,
    /// The name scripts call it by.
    pub name: &'a str,
    /// A method (`%obj.name(..)`), not a function.
    pub method: bool,
    /// A development command ([`ScriptRegistry::dev`]).
    pub dev: bool,
    /// The usage text.
    pub usage: &'a str,
    /// The whole doc comment of a native the attributes declared; empty for one declared by hand.
    pub docs: &'a str,
    /// What it declared it does beyond its result ([`ScriptRegistry::with_effects`]).
    pub effects: Effects,
}

impl Engine {
    /// Every native the registry declared, in order: what a reference page lists with the docs of its declaration. (The
    /// natives a host registers straight into a script engine are not here; its [`Vm::native_listing`] has them all.)
    pub fn natives(&self) -> impl Iterator<Item = NativeEntry<'_>> + '_ {
        self.script_table().filter_map(|entry| match &entry.kind {
            EntryKind::Native {
                namespace,
                name,
                method,
                usage,
                dev,
                docs,
                effects,
                ..
            } => Some(NativeEntry {
                side: entry.side,
                namespace,
                name,
                method: *method,
                dev: *dev,
                usage,
                docs,
                effects: *effects,
            }),
            _ => None,
        })
    }
}

pub(crate) enum EntryKind {
    Native {
        namespace: String,
        name: String,
        method: bool,
        usage: String,
        native: TypedNative,
        dev: bool,
        docs: &'static str,
        effects: Effects,
    },
    Global {
        name: String,
        default: String,
        effects: Effects,
    },
    Pref(&'static crate::PrefDecl, Effects),
    Class(crate::class::ClassInfo),
}

impl ScriptEntry {
    fn install(&self, vm: &mut Vm, engine: &Engine) {
        match &self.kind {
            EntryKind::Native {
                namespace,
                name,
                method,
                usage,
                native,
                dev,
                effects,
                ..
            } => {
                native.install(vm, engine, callee(namespace, name, *method), usage, *dev);
                vm.add_native_effects(namespace, name, *effects);
            }
            EntryKind::Global {
                name,
                default,
                effects,
            } => {
                vm.set_global(name, default);
                vm.add_global_effects(name, *effects);
            }
            EntryKind::Pref(decl, effects) => {
                decl.install(vm);
                vm.add_global_effects(decl.name, *effects);
            }
            EntryKind::Class(class) => class.install(vm, engine),
        }
    }
}

fn callee<'a>(namespace: &'a str, name: &'a str, method: bool) -> Callee<'a> {
    if method {
        Callee::Method {
            class: namespace,
            name,
        }
    } else {
        Callee::Function { namespace, name }
    }
}

/// What the builder keeps for the registry: its entries, and the names they took.
#[derive(Default)]
pub(crate) struct ScriptTable {
    pub(crate) entries: Vec<ScriptEntry>,
    /// Lower-cased `namespace::name` or `$global`: the sides and plugins that took it.
    names: BTreeMap<String, Vec<(Side, PluginId)>>,
}

impl ScriptTable {
    /// Takes `key` (already lower-cased) for `plugin` on `side`; the first plugin that took it on a side this one shares
    /// when it is taken.
    fn take(&mut self, key: String, side: Side, plugin: PluginId) -> Result<(), PluginId> {
        let takers = self.names.entry(key).or_default();
        if let Some(&(_, first)) = takers.iter().find(|(s, _)| s.overlaps(side)) {
            return Err(first);
        }
        takers.push((side, plugin));
        Ok(())
    }
}

/// Whose entries a replay registers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Sides {
    /// Every entry of the stage's side, on a script engine of that side alone.
    Own,
    /// Only the client's own, on a server's script engine its client shares (the entries of both sides are there already).
    Shared,
}

/// Runs the registry's entries for `stage` on `vm`: the server's at [`SERVER_SCRIPT_STAGE`], the client's at
/// [`CLIENT_SCRIPT_STAGE`], in the order the plugins declared them.
pub(crate) fn replay_entries(
    entries: &[ScriptEntry],
    stage: Stage,
    vm: &mut Vm,
    engine: &Engine,
    sides: Sides,
) {
    let side: fn(Side) -> bool = match (stage, sides) {
        (Stage::Server(stage), Sides::Own) if stage == SERVER_SCRIPT_STAGE => Side::server,
        (Stage::Client(stage), Sides::Own) if stage == CLIENT_SCRIPT_STAGE => Side::client,
        (Stage::Client(stage), Sides::Shared) if stage == CLIENT_SCRIPT_STAGE => {
            |side| side == Side::Client
        }
        _ => return,
    };
    for entry in entries.iter().filter(|e| side(e.side)) {
        entry.install(vm, engine);
    }
}

/// The script registry: console functions, methods and globals a plugin declares, from
/// [`EngineBuilder::script`] (every script engine), [`server_script`](EngineBuilder::server_script) or
/// [`client_script`](EngineBuilder::client_script).
///
/// Each is registered into every script engine of its side as the engine makes it, after the recipes of the last stage
/// before scripts run ([`SERVER_SCRIPT_STAGE`], [`CLIENT_SCRIPT_STAGE`]), in plugin order. A name is taken once per
/// side: a second plugin declaring it fails the build ([`BuildError::DuplicateScriptName`], naming both plugins).
/// Names compare without case, as scripts call them.
///
/// The natives are typed ([`IntoNative`]): their argument counts come from the signature, and their usage text is
/// made from it ([`usage_text`]) with the argument names given and the description.
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginId, Res, ResMut, ServerStage};
/// use torque_ts_vm::vm::Vm;
///
/// struct DeadZone(f32);
///
/// /// Sets the stick dead zone (0 to 0.75). Returns the value kept.
/// fn set_dead_zone(mut zone_kept: ResMut<DeadZone>, zone: f32) -> f32 {
///     zone_kept.0 = zone.clamp(0.0, 0.75);
///     zone_kept.0
/// }
///
/// #[derive(PluginId)]
/// struct PadPlugin;
/// impl Plugin for PadPlugin {
///     fn build(&self, engine: &mut EngineBuilder) {
///         engine.insert_resource(DeadZone(0.15));
///         engine
///             .script()
///             .function("Pad", "setDeadZone", &["zone"], "sets the stick dead zone", set_dead_zone)
///             .function("Pad", "getDeadZone", &[], "the stick dead zone", |zone: Res<DeadZone>| zone.0)
///             .global("$Pref::Pad::InvertY", "0");
///     }
/// }
///
/// let engine = EngineBuilder::new().add_plugins(PadPlugin).build()?;
/// let mut vm = Vm::new();
/// for &stage in ServerStage::ALL {
///     engine.replay(stage, &mut vm)?;
/// }
/// assert_eq!(vm.eval(b"return Pad::setDeadZone(2);")?, b"0.75");
/// assert_eq!(vm.eval(b"return Pad::getDeadZone();")?, b"0.75");
/// assert!(vm.native_listing().contains("Pad::setDeadZone 2..2 Pad::setDeadZone(zone) - sets the stick dead zone\n"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ScriptRegistry<'b> {
    engine: &'b mut EngineBuilder,
    side: Side,
    /// The effects of what is being declared inside [`with_effects`](Self::with_effects); none outside it.
    pub(crate) effects: Effects,
}

impl<'b> ScriptRegistry<'b> {
    pub(crate) fn new(engine: &'b mut EngineBuilder, side: Side) -> ScriptRegistry<'b> {
        ScriptRegistry {
            engine,
            side,
            effects: Effects::NONE,
        }
    }

    /// Declares what `declare` declares in this registry with `effects` (on top of what each declaration says itself):
    /// a native that writes or deletes files, runs code from text, switches mods or quits, changes permissions, or
    /// connects to an address it is given. A host's call policy reads them ([`Vm::effects_of_name`]) to keep code it does
    /// not trust (an extension's script language) away from them, so a native with such an effect must say so. Only what
    /// `declare` declares gets them; the registry is as it was before once it returns.
    ///
    /// The globals and prefs `declare` declares get them too, as the effects of writing them ([`Vm::global_effects`]): a
    /// pref that sets a limit, a folder or which mods load is [`Effects::ENGINE`].
    ///
    /// ```
    /// use torque_engine::{Effects, EngineBuilder, Plugin, PluginId, ServerStage};
    /// use torque_ts_vm::vm::Vm;
    ///
    /// #[derive(PluginId)]
    /// struct NotesPlugin;
    /// impl Plugin for NotesPlugin {
    ///     fn build(&self, engine: &mut EngineBuilder) {
    ///         engine
    ///             .script()
    ///             .function("Notes", "count", &[], "how many notes there are", || 0)
    ///             .with_effects(Effects::FILES, |r| {
    ///                 r.function("Notes", "clear", &[], "deletes the notes file", || true);
    ///             })
    ///             .function("Notes", "first", &[], "the first note", || "");
    ///     }
    /// }
    ///
    /// let engine = EngineBuilder::new().add_plugins(NotesPlugin).build()?;
    /// let mut vm = Vm::new();
    /// for &stage in ServerStage::ALL {
    ///     engine.replay(stage, &mut vm)?;
    /// }
    /// assert_eq!(vm.native_effects("Notes", "clear"), Effects::FILES);
    /// assert_eq!(vm.native_effects("Notes", "count"), Effects::NONE);
    /// assert_eq!(vm.native_effects("Notes", "first"), Effects::NONE);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_effects(&mut self, effects: Effects, declare: impl FnOnce(&mut Self)) -> &mut Self {
        let kept = self.effects;
        self.effects = kept | effects;
        declare(self);
        self.effects = kept;
        self
    }

    /// Declares the function `namespace::name` (`namespace` empty for a global function): `f`, with `args` naming its
    /// script arguments and `usage` describing it.
    pub fn function<M>(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[&str],
        usage: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        let native = f.into_native();
        let usage = native.usage(Callee::Function { namespace, name }, args, usage);
        self.native(
            Callee::Function { namespace, name },
            usage,
            native,
            false,
            "",
        )
    }

    /// Declares the method `name` of `class`: called on an object (`%obj.name(..)`), which `f` gets as
    /// [`This`].
    pub fn method<M>(
        &mut self,
        class: &str,
        name: &str,
        args: &[&str],
        usage: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        let native = f.into_native();
        let usage = native.usage(Callee::Method { class, name }, args, usage);
        self.native(Callee::Method { class, name }, usage, native, false, "")
    }

    /// Declares a development command: a [`function`](Self::function) that runs in a debug build, or in a release build
    /// once [`DEV_PREF`] is true; otherwise it prints an error and returns the empty string. It is registered either way,
    /// so the natives a build has do not depend on its profile.
    pub fn dev<M>(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[&str],
        usage: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        let native = f.into_native();
        let usage = native.usage(Callee::Function { namespace, name }, args, usage);
        self.native(
            Callee::Function { namespace, name },
            usage,
            native,
            true,
            "",
        )
    }

    /// Declares the natives the [`function`](crate::function) and [`methods`](crate::methods) attributes made, as
    /// [`natives!`](crate::natives) lists them: each a function, method or development command as its declaration says,
    /// in the list's order.
    ///
    /// ```
    /// use torque_engine::{EngineBuilder, Plugin, PluginId, ResMut, ServerStage, function, natives};
    /// use torque_ts_vm::vm::Vm;
    ///
    /// struct DeadZone(f32);
    ///
    /// /// sets the stick dead zone (0 to 0.75), and returns the value kept
    /// #[function(ns = "Pad")]
    /// fn set_dead_zone(mut kept: ResMut<DeadZone>, zone: f32) -> f32 {
    ///     kept.0 = zone.clamp(0.0, 0.75);
    ///     kept.0
    /// }
    ///
    /// #[derive(PluginId)]
    /// struct PadPlugin;
    /// impl Plugin for PadPlugin {
    ///     fn build(&self, engine: &mut EngineBuilder) {
    ///         engine.insert_resource(DeadZone(0.15));
    ///         engine.script().natives(natives![set_dead_zone]);
    ///     }
    /// }
    ///
    /// let engine = EngineBuilder::new().add_plugins(PadPlugin).build()?;
    /// let mut vm = Vm::new();
    /// for &stage in ServerStage::ALL {
    ///     engine.replay(stage, &mut vm)?;
    /// }
    /// assert_eq!(vm.eval(b"return Pad::setDeadZone(2);")?, b"0.75");
    /// assert!(vm.native_listing().contains(
    ///     "Pad::setDeadZone 2..2 Pad::setDeadZone(zone) - sets the stick dead zone (0 to 0.75), and returns the value kept\n"
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn natives(&mut self, natives: impl IntoIterator<Item = NativeDecl>) -> &mut Self {
        for decl in natives {
            let native = (decl.native)();
            let usage = decl.usage_text(&native);
            let kept = self.effects;
            self.effects = kept | decl.effects;
            self.native(decl.callee(), usage, native, decl.dev, decl.docs);
            self.effects = kept;
        }
        self
    }

    /// Declares a native made at run time ([`TypedNative::from_call`]) as `callee`: what a loaded extension declares
    /// through. `names` name its script arguments; its usage text is `usage` as written, or else made from `names` and
    /// `description` ([`usage_text`]). `docs` are its whole docs for the script reference. A development command when
    /// `dev` ([`dev`](Self::dev)). A name another plugin took, or `names` that do not name each script argument once,
    /// fail the build as the other declarations do.
    #[allow(clippy::too_many_arguments)]
    pub fn native_at_run_time(
        &mut self,
        callee: Callee,
        names: &[&str],
        description: &str,
        usage: Option<&str>,
        dev: bool,
        docs: &'static str,
        native: TypedNative,
    ) -> &mut Self {
        let usage = match usage {
            Some(usage) if names.len() == native.args.len() => Ok(usage.to_string()),
            _ => native.usage(callee, names, description),
        };
        self.native(callee, usage, native, dev, docs)
    }

    /// Declares the global variable `name` (with its `$`) and sets it to `default`, before scripts run.
    pub fn global(&mut self, name: &str, default: &str) -> &mut Self {
        let key = name.to_ascii_lowercase();
        let effects = self.effects;
        self.declare(key, "global", name.to_string(), || EntryKind::Global {
            name: name.to_string(),
            default: default.to_string(),
            effects,
        })
    }

    fn native(
        &mut self,
        callee: Callee,
        usage: Result<String, String>,
        native: TypedNative,
        dev: bool,
        docs: &'static str,
    ) -> &mut Self {
        let (namespace, name) = (callee.namespace(), callee.name());
        let method = matches!(callee, Callee::Method { .. });
        let usage = match usage {
            Ok(usage) => usage,
            Err(message) => {
                let plugin = self.engine.current_plugin();
                self.engine
                    .fail(BuildError::ScriptSignature { plugin, message });
                return self;
            }
        };
        let key = format!("{}::{}", namespace, name).to_ascii_lowercase();
        let kind = if method { "method" } else { "function" };
        let effects = self.effects;
        self.declare(key, kind, callee.label(), || EntryKind::Native {
            namespace: namespace.to_string(),
            name: name.to_string(),
            method,
            usage,
            native,
            dev,
            docs,
            effects,
        })
    }

    pub(crate) fn declare(
        &mut self,
        key: String,
        kind: &'static str,
        shown: String,
        entry: impl FnOnce() -> EntryKind,
    ) -> &mut Self {
        let plugin = self.engine.current_plugin();
        let side = self.side;
        if let Err(first) = self.engine.script_table().take(key, side, plugin) {
            self.engine.fail(BuildError::DuplicateScriptName {
                kind,
                name: shown,
                first,
                second: plugin,
            });
            return self;
        }
        self.engine.script_table().entries.push(ScriptEntry {
            side,
            kind: entry(),
        });
        self
    }
}

/// A native the [`function`](crate::function) or [`methods`](crate::methods) attribute declared: what the registry needs
/// to register it, as a constant. [`natives!`](crate::natives) lists them for [`ScriptRegistry::natives`];
/// [`register`](Self::register) registers one by hand.
#[derive(Clone, Copy, Debug)]
pub struct NativeDecl {
    /// A function's namespace (empty for the global one), or a method's class.
    pub namespace: &'static str,
    /// The name scripts call it by.
    pub name: &'static str,
    /// A method (`%obj.name(..)`), not a function.
    pub method: bool,
    /// A development command ([`ScriptRegistry::dev`]).
    pub dev: bool,
    /// A name for each parameter of the function [`native`](Self::native) makes, in order, injected ones too: the
    /// script arguments' names are the usage text's.
    pub params: &'static [&'static str],
    /// What follows the signature in the usage text (the doc comment's first line); empty for nothing.
    pub description: &'static str,
    /// The usage text exactly as written, in place of the generated one (one kept as it was, such as TGE's).
    pub usage: Option<&'static str>,
    /// The whole doc comment, for the script reference.
    pub docs: &'static str,
    /// What it does beyond its result (the attribute's `effects = "files+code"`): see [`ScriptRegistry::with_effects`].
    pub effects: Effects,
    /// Makes the native.
    pub native: fn() -> TypedNative,
}

impl NativeDecl {
    /// What scripts call it as.
    pub fn callee(&self) -> Callee<'static> {
        callee(self.namespace, self.name, self.method)
    }

    /// Its usage text with `native` (what [`native`](Self::native) made): the kept one, or the one made from its
    /// script arguments' names and its description ([`usage_text`]). Err when its names do not match the parameters.
    pub fn usage_text(&self, native: &TypedNative) -> Result<String, String> {
        let callee = self.callee();
        let names = native.script_names(callee, self.params)?;
        match self.usage {
            Some(usage) => Ok(usage.to_string()),
            None => Ok(usage_text(callee, &names, &native.args, self.description)),
        }
    }

    /// Registers it in `vm` by hand, for a host that makes its script engine without an [`Engine`]'s replay:
    /// `engine` is what its injected parameters come from. Err when its names do not match its parameters.
    pub fn register(&self, vm: &mut Vm, engine: &Engine) -> Result<(), String> {
        let native = (self.native)();
        let usage = self.usage_text(&native)?;
        native.install(vm, engine, self.callee(), &usage, self.dev);
        vm.add_native_effects(self.namespace, self.name, self.effects);
        Ok(())
    }
}

/// A list of declared natives: what [`natives!`](crate::natives) takes. The [`function`](crate::function) attribute
/// implements it for a hidden type named as its function (so the list names the function); the
/// [`methods`](crate::methods) attribute for the impl block's type (or the type its `list = Name` makes).
pub trait Natives {
    /// The natives.
    const NATIVES: &'static [NativeDecl];
}

/// Whether two natives of `lists` take one name (namespace and name, without case, as the registry compares them).
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

/// `a` and `b` alike without ASCII case.
const fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// What a method of a [`methods`](crate::methods) block that takes `&self` or `&mut self` returns to the script: its
/// result, or why its object's data could not be reached.
pub type MethodResult = Result<Native, ScriptError>;

/// Calls `f` with the Rust data of the object `this` and the script engine: how a method of a
/// [`methods`](crate::methods) block that takes `&self` or `&mut self` reaches its object.
///
/// An object's Rust data is its [`SimObject::ext`](torque_ts_vm::sim::SimObject::ext), which Rust code sets (a class's
/// `onAdd`, the code that made the object); it must be a `T`. It is taken out of the object for the call and put back
/// after, so `f` has the data and the whole script engine at once, and no borrow is held across a call back into
/// script: a call that reaches the same object's data again meanwhile (the method's script calling it again) finds
/// none and fails with an error, not a panic. An object deleted during the call drops its data, and so does a method
/// that panics: the data is not put back, so later calls on the object report that it has no data.
///
/// ```
/// use torque_engine::This;
/// use torque_engine::script::with_object_data;
/// use torque_ts_vm::vm::Vm;
///
/// struct Ammo(i32);
///
/// let mut vm = Vm::new();
/// vm.eval(b"new SimObject(Crate);")?;
/// let id = vm.objects().find(b"Crate").expect("the object");
/// vm.objects_mut().get_mut(id).expect("the object").ext = Some(Box::new(Ammo(5)));
/// let left = with_object_data(&mut vm, This(id), |ammo: &mut Ammo, _vm| {
///     ammo.0 -= 1;
///     ammo.0
/// });
/// assert!(left.is_ok());
/// assert!(with_object_data(&mut vm, This(id), |_: &mut String, _vm| 0).is_err(), "not a String");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn with_object_data<T: Any, R: ScriptReturn>(
    vm: &mut Vm,
    this: This,
    f: impl FnOnce(&mut T, &mut Vm) -> R,
) -> MethodResult {
    let id = this.0;
    let Some(object) = vm.objects_mut().get_mut(id) else {
        return Err(ScriptError::new(format!("object {id} is gone")));
    };
    let data = match object.ext.take() {
        Some(data) if data.is::<T>() => data,
        other => {
            object.ext = other;
            return Err(ScriptError::new(format!(
                "object {id} has no `{}` data (or a call on it holds the data)",
                type_name::<T>()
            )));
        }
    };
    let Ok(mut data) = data.downcast::<T>() else {
        return Err(ScriptError::new(format!(
            "object {id} has no `{}` data",
            type_name::<T>()
        )));
    };
    let result = f(&mut data, vm);
    if let Some(object) = vm.objects_mut().get_mut(id)
        && object.ext.is_none()
    {
        object.ext = Some(data);
    }
    result.into_result()
}

/// Lists natives that the [`function`](crate::function) and [`methods`](crate::methods) attributes declared, for
/// [`ScriptRegistry::natives`]: each item a function with `#[function]` (by its path) or a type with a `#[methods]`
/// block (all of its methods), in order, as a `Vec<NativeDecl>`.
///
/// Two natives of one list with the same name (namespace and name, without case) do not compile; one taken by a list
/// and another plugin fails the engine's build ([`BuildError::DuplicateScriptName`]).
///
/// ```
/// use torque_engine::{function, natives};
///
/// /// the sum
/// #[function]
/// fn add(a: i32, b: i32) -> i32 {
///     a + b
/// }
///
/// #[function(ns = "Math")]
/// fn half(n: f32) -> f32 {
///     n / 2.0
/// }
///
/// let list = natives![add, half];
/// assert_eq!(list.iter().map(|n| n.name).collect::<Vec<_>>(), ["add", "half"]);
/// assert_eq!(list[1].namespace, "Math");
/// ```
///
/// Names compare without case, so these two are one name:
///
/// ```compile_fail,E0080
/// use torque_engine::{function, natives};
///
/// #[function(ns = "Pad")]
/// fn dead_zone() -> f32 {
///     0.15
/// }
///
/// #[function(ns = "pad", name = "DEADZONE")]
/// fn other_dead_zone() -> f32 {
///     0.2
/// }
///
/// let _ = natives![dead_zone, other_dead_zone];
/// ```
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
