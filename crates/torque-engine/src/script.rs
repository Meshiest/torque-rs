//! The script registry: console functions, methods and globals that plugins declare, registered into every script engine
//! the engine makes, and typed natives ([`IntoNative`]) whose argument counts and usage text come from their signature.

use std::collections::BTreeMap;
use std::rc::Rc;

use torque_ts_vm::convert::{ArgKind, ScriptError, ScriptReturn};
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use crate::param::{Cx, Param, ParamError, ParamKind, all_arities, shape};
use crate::{BuildError, ClientStage, Engine, EngineBuilder, PluginId, ServerStage, Stage};

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
    args: Vec<ArgKind>,
    make: Rc<MakeNative>,
}

impl TypedNative {
    /// The kinds of its script arguments, in order.
    pub fn args(&self) -> &[ArgKind] {
        &self.args
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
                    let shape = shape(&[$(<$P as Param>::KIND),*]);
                    assert!(shape.vms <= 1, "a native takes at most one `VmMut`");
                    assert!(
                        shape.vms == 0 || shape.vm_reads == 0,
                        "a native takes either one `VmMut` or any number of `VmRef`, never both"
                    );
                    assert!(
                        !shape.misordered,
                        "a native's script arguments go required first, then `Option`s, then one `Rest` last"
                    );
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
    side: Side,
    kind: EntryKind,
}

enum EntryKind {
    Native {
        namespace: String,
        name: String,
        method: bool,
        usage: String,
        native: TypedNative,
        dev: bool,
    },
    Global {
        name: String,
        default: String,
    },
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
            } => native.install(vm, engine, callee(namespace, name, *method), usage, *dev),
            EntryKind::Global { name, default } => vm.set_global(name, default),
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

/// Runs the registry's entries for `stage` on `vm`: the server's at [`SERVER_SCRIPT_STAGE`], the client's at
/// [`CLIENT_SCRIPT_STAGE`], in the order the plugins declared them.
pub(crate) fn replay_entries(entries: &[ScriptEntry], stage: Stage, vm: &mut Vm, engine: &Engine) {
    let side: fn(Side) -> bool = match stage {
        Stage::Server(stage) if stage == SERVER_SCRIPT_STAGE => Side::server,
        Stage::Client(stage) if stage == CLIENT_SCRIPT_STAGE => Side::client,
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
}

impl<'b> ScriptRegistry<'b> {
    pub(crate) fn new(engine: &'b mut EngineBuilder, side: Side) -> ScriptRegistry<'b> {
        ScriptRegistry { engine, side }
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
        self.native(namespace, name, false, args, usage, f.into_native(), false)
    }

    /// Declares the method `name` of `class`: called on an object (`%obj.name(..)`), which `f` gets as
    /// [`This`](crate::This).
    pub fn method<M>(
        &mut self,
        class: &str,
        name: &str,
        args: &[&str],
        usage: &str,
        f: impl IntoNative<M>,
    ) -> &mut Self {
        self.native(class, name, true, args, usage, f.into_native(), false)
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
        self.native(namespace, name, false, args, usage, f.into_native(), true)
    }

    /// Declares the global variable `name` (with its `$`) and sets it to `default`, before scripts run.
    pub fn global(&mut self, name: &str, default: &str) -> &mut Self {
        let key = name.to_ascii_lowercase();
        self.declare(key, "global", name.to_string(), || EntryKind::Global {
            name: name.to_string(),
            default: default.to_string(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn native(
        &mut self,
        namespace: &str,
        name: &str,
        method: bool,
        args: &[&str],
        description: &str,
        native: TypedNative,
        dev: bool,
    ) -> &mut Self {
        let callee = callee(namespace, name, method);
        let usage = match native.usage(callee, args, description) {
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
        self.declare(key, kind, callee.label(), || EntryKind::Native {
            namespace: namespace.to_string(),
            name: name.to_string(),
            method,
            usage,
            native,
            dev,
        })
    }

    fn declare(
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
