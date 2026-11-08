//! What a script language's code may do in a script engine: the engine checks every function and method a language's call
//! runs (`call_function`, `call_method`, and the browser build's `LuaHost`), whatever namespace names it, and every field
//! it writes (`set_field`), against the policy of that script engine. A native extension's own natives are not held to it:
//! their code is native and trusted.

use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::language::VmContext;

/// The console functions [`DenyCodeAndFiles`] refuses: they run code from text, delete, write or export files, or switch
/// mods. Compared without case.
pub const DENIED_FUNCTIONS: &[&str] = &[
    "call",
    "compile",
    "deleteDataBlocks",
    "deleteVariables",
    "disableMod",
    "enableMod",
    "eval",
    "exec",
    "export",
    "fileDelete",
    "loadMods",
    "quit",
    "setModPaths",
];

/// The methods [`DenyCodeAndFiles`] refuses: they write files or call another method by its name. Compared without case.
pub const DENIED_METHODS: &[&str] = &[
    "bindCmd",
    "call",
    "openForAppend",
    "openForRead",
    "openForWrite",
    "save",
    "writeLine",
    "writeObject",
];

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
    /// `Err` with the reason refuses running `namespace::name` (`namespace` empty for a global function; `method` for a
    /// method of an object) with `args`.
    fn check(
        &self,
        namespace: &str,
        name: &str,
        method: bool,
        args: &[Vec<u8>],
    ) -> Result<(), String>;

    /// `Err` with the reason refuses writing `value` to the field `name[index]` of an object (`index` empty for a plain
    /// field). The engine has refused a name or index with a NUL byte before it asks; [`field_name`] is the name the
    /// write will use.
    fn check_field(&self, name: &[u8], index: &[u8], value: &[u8]) -> Result<(), String>;
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
    method: bool,
    args: &'a [Vec<u8>],
) -> Option<(String, bool, &'a [Vec<u8>])> {
    let (at, target_method) = match (method, namespace.is_empty()) {
        (true, _) => (1, true),
        (false, false) => (2, true),
        (false, true) => (2, false),
    };
    let target = args.get(at)?;
    Some((
        String::from_utf8_lossy(target).into_owned(),
        target_method,
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
    method: bool,
    args: &[Vec<u8>],
) -> Option<Vec<u8>> {
    let spec = name.eq_ignore_ascii_case("setFieldValue");
    let apart = name.eq_ignore_ascii_case("setDataField");
    if !spec && !apart {
        return None;
    }
    let firsts: &[usize] = match (method, namespace.is_empty()) {
        (true, _) => &[0],
        (false, false) => &[1],
        (false, true) => &[0, 1],
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

/// The default policy: everything but [`DENIED_FUNCTIONS`] and [`DENIED_METHODS`], under any namespace (a method is a
/// namespace's function too: `SimObject::save`), and no `schedule` that names one of them in any argument.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyCodeAndFiles;

/// Whether `name` is denied, without case.
fn denied(name: &[u8]) -> bool {
    DENIED_FUNCTIONS
        .iter()
        .chain(DENIED_METHODS)
        .any(|d| d.as_bytes().eq_ignore_ascii_case(name))
}

impl CallPolicy for DenyCodeAndFiles {
    fn check(
        &self,
        namespace: &str,
        name: &str,
        method: bool,
        args: &[Vec<u8>],
    ) -> Result<(), String> {
        let shown = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{namespace}::{name}")
        };
        if denied(name.as_bytes()) {
            return Err(format!("`{shown}` cannot be called from a script language"));
        }
        if let Some(field) = written_code_field(namespace, name, method, args) {
            return Err(format!(
                "`{shown}` cannot set `{}` from a script language: the engine runs that field's text",
                String::from_utf8_lossy(&field)
            ));
        }
        // What stores a function's or a method's name to call later: none may name a denied one.
        if STORE_NAMES.iter().any(|s| s.eq_ignore_ascii_case(name))
            && let Some(target) = args.iter().find(|a| denied(a))
        {
            return Err(format!(
                "`{shown}` cannot queue `{}` from a script language",
                String::from_utf8_lossy(target)
            ));
        }
        // A `schedule`'s call is checked now as it will be when it fires (`DeferredCalls` checks it again then).
        if name.eq_ignore_ascii_case("schedule")
            && let Some((target, target_method, target_args)) = scheduled(namespace, method, args)
        {
            let (target_ns, target_name) = target.rsplit_once("::").unwrap_or(("", &target));
            self.check(target_ns, target_name, target_method, target_args)
                .map_err(|why| format!("`{shown}` cannot queue that call: {why}"))?;
        }
        Ok(())
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
    method: bool,
    args: &[Vec<u8>],
) -> Result<(), String> {
    VmContext::of(vm)
        .policy()
        .check(namespace, name, method, args)
}
