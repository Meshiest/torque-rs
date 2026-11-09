//! What a script language's code may do in a script engine: the engine checks every function and method a language's call
//! runs (`call_function`, `call_method`, and the browser build's `LuaHost`), whatever namespace names it, every field it
//! writes (`set_field`) and every global it writes (`set_global`), against the policy of that script engine. A native extension's own natives are not held to it:
//! their code is native and trusted.
//!
//! What a native does is declared where it is registered ([`Effects`], kept by the script engine): the default policy
//! refuses every native that declared one of [`DENIED_EFFECTS`], so a native added later with such an effect is refused as
//! soon as it says so. [`TGE_EFFECTS`] covers the TGE names a host may register without declaring anything. A global's
//! write has effects too ([`Vm::add_global_effects`], and a registry's `with_effects` on the prefs it declares), with
//! [`GLOBAL_EFFECTS`] for the engine's globals read without a declaration.

use std::rc::Rc;
use std::sync::LazyLock;

use torque_ts_vm::effects::{Effects, GlobalPattern, global_effects_in};
use torque_ts_vm::vm::Vm;

use crate::language::VmContext;

/// The effects [`DenyCodeAndFiles`] refuses: files, code, the engine as a whole and permissions. A network effect is
/// allowed (the queries and the connection to a server are a mod's to make), and so is writing a variable an argument
/// names, which is checked as a write of that variable.
pub const DENIED_EFFECTS: Effects = Effects::FILES
    .union(Effects::CODE)
    .union(Effects::ENGINE)
    .union(Effects::PERMISSIONS);

/// TGE's functions and methods with effects, by name: a call is held to these effects whatever the script engine
/// declared, so a host that registers one of them without declaring its effects is still covered. Compared without case.
pub const TGE_EFFECTS: &[(&str, Effects)] = &[
    ("activatePackage", Effects::ENGINE),
    ("bindCmd", Effects::CODE),
    ("call", Effects::CODE),
    ("compile", Effects::CODE.union(Effects::FILES)),
    ("deactivatePackage", Effects::ENGINE),
    ("deleteDataBlocks", Effects::ENGINE),
    ("deleteVariables", Effects::ENGINE),
    ("disableMod", Effects::ENGINE),
    ("enableMod", Effects::ENGINE),
    ("eval", Effects::CODE),
    ("exec", Effects::CODE),
    ("export", Effects::FILES),
    ("fileDelete", Effects::FILES),
    ("loadMods", Effects::ENGINE),
    ("nextToken", Effects::VARIABLES),
    ("openForAppend", Effects::FILES),
    ("openForRead", Effects::FILES),
    ("openForWrite", Effects::FILES),
    ("quit", Effects::ENGINE),
    ("save", Effects::FILES),
    ("setModPaths", Effects::ENGINE),
    ("writeLine", Effects::FILES),
    ("writeObject", Effects::FILES),
];

/// The engine's globals whose writes have effects, read where no declaration names them (a start of names when a pattern
/// ends in `*`; compared without case, as [`GlobalPattern`] compares): the realms' own limits, the switch of the
/// development commands, a server's limits, password and LAN mode, the network's rates and sizes, and which mods load.
pub const GLOBAL_EFFECTS: &[(&str, Effects)] = &[
    ("$Con::*", Effects::ENGINE),
    ("$instantGroup", Effects::ENGINE),
    ("$Pref::Dev", Effects::ENGINE),
    ("$Pref::Lua::*", Effects::ENGINE),
    ("$Pref::Mods*", Effects::ENGINE),
    ("$pref::Net::*", Effects::ENGINE),
    ("$Pref::Server::*", Effects::ENGINE),
    ("$Server::LAN", Effects::ENGINE),
];

/// [`GLOBAL_EFFECTS`], each pattern made once.
static LISTED_GLOBALS: LazyLock<Vec<(GlobalPattern, Effects)>> = LazyLock::new(|| {
    GLOBAL_EFFECTS
        .iter()
        .map(|&(pattern, effects)| (GlobalPattern::new(pattern), effects))
        .collect()
});

/// The effects of writing the global `name` (with its `$`) in `vm`: what was declared, and [`GLOBAL_EFFECTS`].
pub fn global_effects(vm: &Vm, name: &[u8]) -> Effects {
    vm.global_effects(name) | global_effects_in(&LISTED_GLOBALS, name)
}

/// Whether a call runs a function or an object's method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// A function (`namespace::name(...)`, `namespace` empty for a global one).
    Function,
    /// A method of an object (`obj.name(...)`); its arguments do not hold the object.
    Method,
}

impl CallKind {
    /// [`Method`](Self::Method) when `method`, else [`Function`](Self::Function).
    pub fn of(method: bool) -> CallKind {
        if method {
            CallKind::Method
        } else {
            CallKind::Function
        }
    }
}

/// A global's write a policy decides on: the name (with its `$`), the value, and the effects its script engine knows of.
#[derive(Clone, Copy)]
pub struct GlobalWrite<'a> {
    /// The global's name, with its `$`.
    pub name: &'a str,
    /// The value written.
    pub value: &'a [u8],
    vm: &'a Vm,
}

impl<'a> GlobalWrite<'a> {
    /// The write of `value` to `name` in `vm`.
    pub fn new(vm: &'a Vm, name: &'a str, value: &'a [u8]) -> GlobalWrite<'a> {
        GlobalWrite { name, value, vm }
    }

    /// The effects of the write ([`global_effects`]).
    pub fn effects(&self) -> Effects {
        global_effects(self.vm, self.name.as_bytes())
    }
}

/// A call a policy decides on: what runs, with which arguments, and the effects its script engine knows of.
#[derive(Clone, Copy)]
pub struct PolicyCall<'a> {
    /// Empty for a global function.
    pub namespace: &'a str,
    /// The function's or the method's name.
    pub name: &'a str,
    /// A function or a method.
    pub kind: CallKind,
    /// Its arguments (a method's without the object).
    pub args: &'a [Vec<u8>],
    vm: &'a Vm,
}

impl<'a> PolicyCall<'a> {
    /// The call of `namespace::name` with `args` in `vm`.
    pub fn new(
        vm: &'a Vm,
        namespace: &'a str,
        name: &'a str,
        kind: CallKind,
        args: &'a [Vec<u8>],
    ) -> PolicyCall<'a> {
        PolicyCall {
            namespace,
            name,
            kind,
            args,
            vm,
        }
    }

    /// Another call in the same script engine (a `schedule`'s target, say).
    #[must_use]
    pub fn other(
        &self,
        namespace: &'a str,
        name: &'a str,
        kind: CallKind,
        args: &'a [Vec<u8>],
    ) -> PolicyCall<'a> {
        PolicyCall::new(self.vm, namespace, name, kind, args)
    }

    /// The effects of what it runs: those any native of its name declared in any namespace, and [`TGE_EFFECTS`]'.
    pub fn effects(&self) -> Effects {
        self.effects_of(self.name.as_bytes())
    }

    /// The effects of a native called `name`, as [`effects`](Self::effects) reads them.
    pub fn effects_of(&self, name: &[u8]) -> Effects {
        effects_of(self.vm, name)
    }

    /// The write of `value` to the global `name` in the call's script engine (a native that writes a global it is
    /// given the name of: `nextToken`).
    pub fn global_write(&self, name: &'a str, value: &'a [u8]) -> GlobalWrite<'a> {
        GlobalWrite::new(self.vm, name, value)
    }

    /// The globals a call with [`Effects::VARIABLES`] may write: each argument read as a variable's name, as `nextToken`
    /// reads its own (a language's call runs in no frame, so a name is a global's: `$` is put before one without it).
    pub fn named_globals(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| {
                let name = String::from_utf8_lossy(arg);
                if name.starts_with('$') {
                    name.into_owned()
                } else {
                    format!("${name}")
                }
            })
            .collect()
    }
}

/// Err when a name the call gives has a NUL byte: its namespace or name, and every argument of a call that stores a
/// name ([`STORE_NAMES`]) or writes a variable an argument names ([`Effects::VARIABLES`]). The engine keeps a name only up
/// to its NUL, so a policy that compared the whole name would see another name than the one used. Refused whatever the
/// policy.
pub fn refuse_nul_names(call: &PolicyCall<'_>) -> Result<(), String> {
    let refused = || {
        Err(format!(
            "`{}` cannot be called from a script language with a NUL byte in a name",
            String::from_utf8_lossy(&until_nul(call.name.as_bytes()))
        ))
    };
    if call.namespace.contains('\0') || call.name.contains('\0') {
        return refused();
    }
    let names_args = STORE_NAMES
        .iter()
        .any(|s| s.eq_ignore_ascii_case(call.name))
        || call.effects().contains(Effects::VARIABLES);
    if names_args && call.args.iter().any(|a| a.contains(&0)) {
        return refused();
    }
    Ok(())
}

/// `text` up to its first NUL byte.
fn until_nul(text: &[u8]) -> Vec<u8> {
    torque_ts_vm::text::until_nul(text).to_vec()
}

/// The effects of any native called `name` in `vm`: what it declared, and [`TGE_EFFECTS`].
pub fn effects_of(vm: &Vm, name: &[u8]) -> Effects {
    let tge = TGE_EFFECTS
        .iter()
        .find(|(n, _)| n.as_bytes().eq_ignore_ascii_case(name))
        .map_or(Effects::NONE, |&(_, e)| e);
    vm.effects_of_name(name) | tge
}

/// The fields [`DenyCodeAndFiles`] refuses a language to write: the engine evaluates their text as script (a control's
/// `command` when it is clicked, a text edit's `validate`, a console text's `expression`). A write is refused when one of
/// them, without case, starts the field name the engine builds from the name and the index
/// ([`torque_ts_vm::sim::dynamic_name`]): `command`, `Command0`, `command[1]` alike. `setFieldValue` naming one is
/// refused too.
pub const CODE_FIELDS: &[&str] = &[
    "altCommand",
    "closeCommand",
    "command",
    "escapeCommand",
    "expression",
    "validate",
];

/// Decides which functions and methods a script language's code may run in one script engine, and which fields it may
/// write.
pub trait CallPolicy {
    /// `Err` with the reason refuses the call.
    fn check(&self, call: &PolicyCall<'_>) -> Result<(), String>;

    /// `Err` with the reason refuses writing `value` to the field `name[index]` of an object (`index` empty for a plain
    /// field). The engine has refused a name or index with a NUL byte before it asks; [`field_name`] is the name the
    /// write will use.
    fn check_field(&self, name: &[u8], index: &[u8], value: &[u8]) -> Result<(), String>;

    /// `Err` with the reason refuses writing a global. The engine has checked the name is a global's before it asks.
    fn check_global(&self, write: &GlobalWrite<'_>) -> Result<(), String>;
}

/// The field name the engine builds for `name[index]`, the one a write uses ([`torque_ts_vm::sim::dynamic_name`]).
pub fn field_name(name: &[u8], index: &[u8]) -> Vec<u8> {
    torque_ts_vm::sim::dynamic_name(name, index)
}

/// The functions and methods that keep the name of a function or method to call later: `schedule`, and an action map's
/// binds (a key calls the bound function).
const STORE_NAMES: &[&str] = &["bind", "bindObj", "schedule"];

/// The call a `schedule` queues: its function or method, whether that is a method, and its arguments. As a method
/// (`obj.schedule(delay, method, ...)`) the method comes second; as a namespace's function (`SimObject::schedule(obj,
/// delay, method, ...)`) third; as the global function (`schedule(delay, object, function, ...)`) third, a function.
pub fn scheduled<'a>(
    namespace: &str,
    kind: CallKind,
    args: &'a [Vec<u8>],
) -> Option<(String, CallKind, &'a [Vec<u8>])> {
    let (at, target) = match (kind, namespace.is_empty()) {
        (CallKind::Method, _) => (1, CallKind::Method),
        (CallKind::Function, false) => (2, CallKind::Method),
        (CallKind::Function, true) => (2, CallKind::Function),
    };
    let name = args.get(at)?;
    Some((
        String::from_utf8_lossy(name).into_owned(),
        target,
        args.get(at + 1..).unwrap_or(&[]),
    ))
}

/// The field a call to a field setter would write, when it is a code field or its name has a NUL: `setFieldValue(spec,
/// value)` with a `"name[index]"` spec (read as the engine reads one, [`torque_ts_vm::sim::split_field_spec`]), and
/// `setDataField(slot, array, value)` with the name and the index apart. As a method the object is not among `args`; as a
/// namespace's function (`SimObject::setDataField(obj, ...)`) it comes first, so the arguments shift by one; a global
/// function of that name is checked both ways. The name checked is the engine's own join ([`field_name`]).
fn written_code_field(
    namespace: &str,
    name: &str,
    kind: CallKind,
    args: &[Vec<u8>],
) -> Option<Vec<u8>> {
    let spec = name.eq_ignore_ascii_case("setFieldValue");
    let apart = name.eq_ignore_ascii_case("setDataField");
    if !spec && !apart {
        return None;
    }
    let firsts: &[usize] = match (kind, namespace.is_empty()) {
        (CallKind::Method, _) => &[0],
        (CallKind::Function, false) => &[1],
        (CallKind::Function, true) => &[0, 1],
    };
    let empty = Vec::new();
    for &at in firsts {
        let (name, index) = if spec {
            let given = args.get(at).unwrap_or(&empty);
            if given.contains(&0) {
                return Some(given.clone());
            }
            torque_ts_vm::sim::split_field_spec(given)
        } else {
            let (slot, array) = (
                args.get(at).unwrap_or(&empty),
                args.get(at + 1).unwrap_or(&empty),
            );
            if slot.contains(&0) || array.contains(&0) {
                return Some([slot.as_slice(), array].concat());
            }
            (slot.as_slice(), array.as_slice())
        };
        let built = field_name(name, index);
        if code_field(&built) {
            return Some(built);
        }
    }
    None
}

/// Whether a code field ([`CODE_FIELDS`]) starts `name`, without case.
fn code_field(name: &[u8]) -> bool {
    CODE_FIELDS.iter().any(|f| {
        name.get(..f.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(f.as_bytes()))
    })
}

/// The default policy: no native whose name has one of [`DENIED_EFFECTS`] in any namespace (a method is a namespace's
/// function too: `SimObject::save`), no write of a code field, and no `schedule` or bind that names a denied native in any
/// argument.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyCodeAndFiles;

/// The denied effects of a native called `name`, or of the function a `Namespace::name` text names.
fn denied(call: &PolicyCall<'_>, name: &[u8]) -> Effects {
    let short = name
        .windows(2)
        .rposition(|w| w == b"::")
        .map_or(name, |at| &name[at + 2..]);
    (call.effects_of(name) | call.effects_of(short)).intersection(DENIED_EFFECTS)
}

impl CallPolicy for DenyCodeAndFiles {
    fn check(&self, call: &PolicyCall<'_>) -> Result<(), String> {
        refuse_nul_names(call)?;
        let PolicyCall {
            namespace,
            name,
            kind,
            args,
            ..
        } = *call;
        let shown = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{namespace}::{name}")
        };
        let effects = denied(call, name.as_bytes());
        if !effects.is_empty() {
            return Err(format!(
                "`{shown}` cannot be called from a script language: its effects ({effects}) are refused"
            ));
        }
        if let Some(field) = written_code_field(namespace, name, kind, args) {
            return Err(format!(
                "`{shown}` cannot set `{}` from a script language: the engine runs that field's text",
                String::from_utf8_lossy(&field)
            ));
        }
        // What stores a function's or a method's name to call later: none may name a denied one.
        if STORE_NAMES.iter().any(|s| s.eq_ignore_ascii_case(name))
            && let Some(target) = args.iter().find(|a| !denied(call, a).is_empty())
        {
            return Err(format!(
                "`{shown}` cannot queue `{}` from a script language",
                String::from_utf8_lossy(target)
            ));
        }
        // A native that writes a variable an argument names (`nextToken`'s token): each argument may be that name, and
        // each is checked as a write of that global.
        if call.effects().contains(Effects::VARIABLES) {
            for global in call.named_globals() {
                self.check_global(&call.global_write(&global, &[]))
                    .map_err(|why| format!("`{shown}` cannot write that variable: {why}"))?;
            }
        }
        // A `schedule`'s call is checked now as it will be when it fires (`DeferredCalls` checks it again then).
        if name.eq_ignore_ascii_case("schedule")
            && let Some((target, target_kind, target_args)) = scheduled(namespace, kind, args)
        {
            let (target_ns, target_name) = target.rsplit_once("::").unwrap_or(("", &target));
            self.check(&call.other(target_ns, target_name, target_kind, target_args))
                .map_err(|why| format!("`{shown}` cannot queue that call: {why}"))?;
        }
        Ok(())
    }

    fn check_global(&self, write: &GlobalWrite<'_>) -> Result<(), String> {
        let effects = write.effects().intersection(DENIED_EFFECTS);
        if effects.is_empty() {
            return Ok(());
        }
        Err(format!(
            "the global `{}` cannot be set from a script language: its effects ({effects}) are refused",
            write.name
        ))
    }

    fn check_field(&self, name: &[u8], index: &[u8], _value: &[u8]) -> Result<(), String> {
        let built = field_name(name, index);
        if code_field(&built) {
            return Err(format!(
                "the field `{}` cannot be set from a script language: the engine runs its text",
                String::from_utf8_lossy(&built)
            ));
        }
        Ok(())
    }
}

/// Puts `policy` in force for the script languages of `vm` (by default [`DenyCodeAndFiles`]): a realm with other rights
/// (an allowlist, say) installs its own.
pub fn set_call_policy(vm: &mut Vm, policy: Rc<dyn CallPolicy>) {
    VmContext::of(vm).set_policy(policy);
}

/// The policy's answer for a language's write of `value` to the global `name` (a global's name, checked) in `vm`: a name
/// with a NUL byte is refused whatever the policy, as [`check_field`] refuses one.
pub(crate) fn check_global(vm: &mut Vm, name: &str, value: &[u8]) -> Result<(), String> {
    if name.contains('\0') {
        return Err(
            "a global name with a NUL byte cannot be set from a script language".to_string(),
        );
    }
    let policy = VmContext::of(vm).policy();
    policy.check_global(&GlobalWrite::new(vm, name, value))
}

/// The policy's answer for a language's write of `name[index]` in `vm`: a NUL byte in the name or the index is refused
/// whatever the policy (the engine would cut the name there, so the policy would see another name than the write uses).
pub(crate) fn check_field(
    vm: &mut Vm,
    name: &[u8],
    index: &[u8],
    value: &[u8],
) -> Result<(), String> {
    if name.contains(&0) || index.contains(&0) {
        return Err(
            "a field name with a NUL byte cannot be set from a script language".to_string(),
        );
    }
    VmContext::of(vm).policy().check_field(name, index, value)
}

/// Notes the call `namespace::name` a language just ran in `vm` with the result `result`: a `schedule` (either form)
/// answers its event's number, whose call is checked again by the policy in force now when it fires.
pub(crate) fn note_deferred(vm: &mut Vm, name: &str, result: &[u8]) {
    if !name.eq_ignore_ascii_case("schedule") {
        return;
    }
    if let Some(seq) = std::str::from_utf8(result)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .filter(|&seq| seq != 0)
    {
        let context = VmContext::of(vm);
        context.defer(vm, seq);
    }
}

/// The policy's answer for `vm`.
pub(crate) fn check(
    vm: &mut Vm,
    namespace: &str,
    name: &str,
    kind: CallKind,
    args: &[Vec<u8>],
) -> Result<(), String> {
    let policy = VmContext::of(vm).policy();
    let call = PolicyCall::new(vm, namespace, name, kind, args);
    refuse_nul_names(&call)?;
    policy.check(&call)
}
