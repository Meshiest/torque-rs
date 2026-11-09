//! Script languages of extensions in the engine: the runner `exec` hands their files to, and what the engine keeps for
//! each script engine (its number, its side, the packages its files declare into, and who to tell when it goes).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use torque_engine::Side;
use torque_ts_vm::policy::ScriptLanguage;
use torque_ts_vm::vm::{Vm, VmError};

use crate::call_policy::{CallPolicy, DenyCodeAndFiles};
use crate::extension::ExtensionState;

/// A queued call of a language that is running: its event, its policy, and the next event number when it started.
/// The policies a queued call is held to: every one recorded for it must allow it (so recording one more can only make
/// a call stricter, never looser).
type Policies = Vec<Rc<dyn CallPolicy>>;

type Inherited = (u32, Policies, u32);

/// What the engine keeps beside one script engine for its extensions' languages: a `Vm` extension, dropped with it.
pub(crate) struct VmContext {
    /// Its number ([`TorqueVm`](torque_plugin_abi::TorqueVm)): never 0, never reused.
    pub(crate) id: u64,
    side: RefCell<Side>,
    /// The packages its languages' files were given ([`TorqueApi::script_package`]), which their definitions may go into.
    ///
    /// [`TorqueApi::script_package`]: torque_plugin_abi::TorqueApi::script_package
    packages: RefCell<BTreeSet<Vec<u8>>>,
    /// The languages to tell when it goes.
    languages: RefCell<Vec<(Arc<ExtensionState>, usize)>>,
    /// What its languages' code may call.
    policy: RefCell<Rc<dyn CallPolicy>>,
    /// The calls its languages queued (`schedule`), by event, with the policies in force then: checked again as they fire.
    deferred: RefCell<std::collections::HashMap<u32, Policies>>,
    /// The queued calls of languages running now (their event, their policy, and the next event number when they
    /// started): what they queue in turn is the language's too, under the same policy.
    inherited: RefCell<Vec<Inherited>>,
}

impl VmContext {
    /// The context of `vm`, made (with no side) when it has none.
    pub(crate) fn of(vm: &mut Vm) -> Rc<VmContext> {
        if let Some(context) = vm.extension::<VmContext>() {
            return context;
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let context = Rc::new(VmContext {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            side: RefCell::new(Side::Both),
            packages: RefCell::new(BTreeSet::new()),
            languages: RefCell::new(Vec::new()),
            policy: RefCell::new(Rc::new(DenyCodeAndFiles)),
            deferred: RefCell::new(std::collections::HashMap::new()),
            inherited: RefCell::new(Vec::new()),
        });
        vm.set_extension(Rc::clone(&context));
        vm.set_fire_check(Some(Rc::new(fire_check)));
        vm.set_fire_done(Some(Rc::new(fire_done)));
        context
    }

    /// The side: [`Side::Both`] for a script engine no server or client recipe made.
    pub(crate) fn side(&self) -> Side {
        *self.side.borrow()
    }

    /// What its languages' code may call.
    pub(crate) fn policy(&self) -> Rc<dyn CallPolicy> {
        Rc::clone(&self.policy.borrow())
    }

    /// Keeps the policy in force for the scheduled call `seq` a language queued, to check it again as it fires.
    pub(crate) fn defer(&self, vm: &mut Vm, seq: u32) {
        let policies = vec![self.policy()];
        record_deferred(self, vm, &[seq], &policies);
    }

    /// Puts another policy in force.
    pub(crate) fn set_policy(&self, policy: Rc<dyn CallPolicy>) {
        *self.policy.borrow_mut() = policy;
    }

    /// Sets the side, once: a script engine made by a server's or a client's recipes.
    pub(crate) fn set_side(&self, side: Side) {
        if self.side() == Side::Both {
            *self.side.borrow_mut() = side;
        }
    }

    /// Lets the definitions of `vm`'s languages go into `package`.
    pub(crate) fn allow_package(&self, package: &[u8]) {
        self.packages
            .borrow_mut()
            .insert(package.to_ascii_lowercase());
    }

    /// Whether `package` was given to a language's file in this script engine.
    pub(crate) fn package_allowed(&self, package: &[u8]) -> bool {
        self.packages
            .borrow()
            .contains(&package.to_ascii_lowercase())
    }
}

/// How many queued calls of languages a script engine keeps before it forgets the ones no longer pending.
const MAX_DEFERRED: usize = 4096;

/// The most policies one queued call is held to; a call that would need more is cancelled.
const MAX_POLICIES: usize = 4;

/// The engine's check of a scheduled call as it fires: a call a language queued is held to the policy that was in force
/// when it was queued, on its actual function and arguments; it is skipped, with a warning, when refused.
fn fire_check(vm: &mut Vm, event: &torque_ts_vm::events::Event) -> bool {
    let torque_ts_vm::events::EventKind::Call { argv, on_object } = &event.kind else {
        return true;
    };
    let Some(context) = vm.extension::<VmContext>() else {
        return true;
    };
    let Some(policies) = context.deferred.borrow_mut().remove(&event.seq) else {
        return true;
    };
    let Some((name, args)) = argv.split_first() else {
        return true;
    };
    let name = String::from_utf8_lossy(name);
    let (namespace, short) = name.rsplit_once("::").unwrap_or(("", &name));
    let kind = crate::call_policy::CallKind::of(*on_object);
    let call = crate::call_policy::PolicyCall::new(vm, namespace, short, kind, args);
    let checked = crate::call_policy::refuse_nul_names(&call)
        .and_then(|()| policies.iter().try_for_each(|policy| policy.check(&call)));
    match checked {
        Ok(()) => {
            let next = vm.events().next_seq();
            context
                .inherited
                .borrow_mut()
                .push((event.seq, policies, next));
            true
        }
        Err(why) => {
            vm.warn(format!(
                "a scheduled call of a script language did not run: {why}"
            ));
            false
        }
    }
}

/// After a queued call of a language ran: every call it queued in turn (the events numbered since it started that are
/// still pending) is the language's, recorded under the same policy, to be checked as it fires.
fn fire_done(vm: &mut Vm, seq: u32) {
    let Some(context) = vm.extension::<VmContext>() else {
        return;
    };
    let Some(at) = context
        .inherited
        .borrow()
        .iter()
        .rposition(|(s, _, _)| *s == seq)
    else {
        return;
    };
    let (_, policies, start) = context.inherited.borrow_mut().remove(at);
    let end = vm.events().next_seq();
    let mut queued = Vec::new();
    let mut seq = start;
    while seq != end {
        if vm.event_pending(seq) {
            queued.push(seq);
        }
        seq = seq.wrapping_add(1);
    }
    record_deferred(&context, vm, &queued, &policies);
}

/// Adds `policies` to each of a language's queued calls `seqs`, to be checked as they fire: a call keeps every policy
/// recorded for it and must pass them all. Calls cancelled or dropped with their object never fire, so a full table is
/// cleared of them once; a call that still finds no room, or would need more than [`MAX_POLICIES`], is cancelled (it
/// would otherwise fire with fewer checks than it was given).
fn record_deferred(
    context: &VmContext,
    vm: &mut Vm,
    seqs: &[u32],
    policies: &[Rc<dyn CallPolicy>],
) {
    let mut cancelled = Vec::new();
    {
        let mut deferred = context.deferred.borrow_mut();
        if deferred.len() + seqs.len() > MAX_DEFERRED {
            deferred.retain(|&seq, _| vm.event_pending(seq));
        }
        for &seq in seqs {
            if !deferred.contains_key(&seq) && deferred.len() >= MAX_DEFERRED {
                cancelled.push(seq);
                continue;
            }
            let held = deferred.entry(seq).or_default();
            for policy in policies {
                if held.iter().any(|h| Rc::ptr_eq(h, policy)) {
                    continue;
                }
                if held.len() >= MAX_POLICIES {
                    cancelled.push(seq);
                    break;
                }
                held.push(Rc::clone(policy));
            }
        }
        for seq in &cancelled {
            deferred.remove(seq);
        }
    }
    if !cancelled.is_empty() {
        for seq in &cancelled {
            vm.cancel_script_event(*seq);
        }
        vm.warn(format!(
            "{} scheduled calls of a script language were cancelled: more than {MAX_DEFERRED} are waiting",
            cancelled.len()
        ));
    }
}

impl Drop for VmContext {
    fn drop(&mut self) {
        let languages = std::mem::take(&mut *self.languages.borrow_mut());
        for (state, lang) in languages {
            state.vm_dropped(lang, self.id);
        }
    }
}

/// The [`ScriptLanguage`] of one language of an extension, installed in each script engine of its side.
pub(crate) struct LanguageRunner {
    state: Arc<ExtensionState>,
    lang: usize,
}

impl LanguageRunner {
    /// Installs the language `lang` of `state` in `vm`, a script engine of `side`: once (a second stage's install does
    /// nothing more).
    pub(crate) fn install(vm: &mut Vm, state: &Arc<ExtensionState>, lang: usize, side: Side) {
        let context = VmContext::of(vm);
        context.set_side(side);
        let spec = &state.languages[lang];
        {
            let mut languages = context.languages.borrow_mut();
            if languages
                .iter()
                .any(|(s, l)| Arc::ptr_eq(s, state) && *l == lang)
            {
                return;
            }
            languages.push((Arc::clone(state), lang));
        }
        let runner = Rc::new(LanguageRunner {
            state: Arc::clone(state),
            lang,
        });
        if !vm.set_script_language(&spec.extension, runner) {
            vm.warn(format!(
                "native extension {}: `.{}` files cannot be its language's: TorqueScript's or another language's",
                state.id, spec.extension
            ));
        }
    }
}

impl ScriptLanguage for LanguageRunner {
    fn run(&self, vm: &mut Vm, path: &[u8], source: &[u8]) -> Result<(), VmError> {
        self.state.run_file(self.lang, vm, path, source)
    }
}

/// A console name: ASCII letters, digits and `_`, not starting with a digit.
pub(crate) fn is_script_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| !b.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A global's name: `$` and then letters, digits, `_` and `::` separators.
pub(crate) fn is_global_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('$') else {
        return false;
    };
    !rest.is_empty() && rest.split("::").all(is_script_name)
}

/// Whether `path` is a relative path of the game's files with no `..`, `.` or empty part, no drive or backslash.
pub(crate) fn is_plain_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// The script file `path` (checked with [`is_plain_path`] first) after the checks `exec` makes: `None` when the exec
/// policy refuses it (a file a server sent) or it is missing.
pub(crate) fn read_checked(vm: &mut Vm, path: &str) -> Result<Option<Vec<u8>>, VmError> {
    if let Some(policy) = vm.exec_policy()
        && !policy.check(vm, path.as_bytes())?
    {
        return Ok(None);
    }
    vm.find_file(path.as_bytes())
}
