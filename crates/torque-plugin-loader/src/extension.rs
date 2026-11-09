//! A loaded extension in the engine: its state (disabled after a panic), the plugin that adds what it registered, and the
//! handle hosts and tests read it through.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use torque_engine::script::{Callee, TypedNative};
use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginId, PluginType, ServerStage, Side};
use torque_plugin_abi::{TORQUE_MAX_NESTING, TorquePluginInitFn};
use torque_ts_vm::convert::ScriptError;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::ffi::{self, CallOutcome, Registered};
use crate::language::{LanguageRunner, VmContext};
use crate::manifest::Manifest;

/// The most log lines an extension's state keeps (the oldest go).
const MAX_LOG_LINES: usize = 256;

/// A loaded extension's state, shared by its natives and passes.
pub(crate) struct ExtensionState {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) sha256: Option<String>,
    pub(crate) library: Option<PathBuf>,
    pub(crate) loaded_from: Option<PathBuf>,
    pub(crate) net_version: Option<u32>,
    pub(crate) natives: Vec<ffi::NativeSpec>,
    pub(crate) globals: Vec<ffi::GlobalSpec>,
    pub(crate) passes: Vec<ffi::PassSpec>,
    pub(crate) languages: Vec<ffi::LanguageSpec>,
    /// Why it is disabled, once it is.
    disabled: Mutex<Option<String>>,
    /// Whether `disabled` holds a reason: what every call reads, so that a call takes no lock for it.
    off: AtomicBool,
    /// Held for each call into it: one at a time.
    calls: Mutex<()>,
    /// The thread holding `calls` ([`thread_token`]; 0 for none).
    owner: AtomicU64,
    /// How many calls into it are live on the owner's thread (nested ones too).
    nesting: AtomicUsize,
    /// How many of those are calling the engine back ([`call_out`](Self::call_out)): a nested call is allowed only while
    /// every live one is.
    calling_out: AtomicUsize,
    /// Where the stack was at each live call into it, outermost first (`stack_mark`): what a call back is charged by.
    marks: Mutex<Vec<usize>>,
    /// The steps the outermost call charged when its script engine has no step limit ([`TORQUE_DEFAULT_STEPS`]).
    ///
    /// [`TORQUE_DEFAULT_STEPS`]: torque_plugin_abi::TORQUE_DEFAULT_STEPS
    pub(crate) fallback_steps: AtomicU64,
    log: Mutex<Vec<String>>,
    /// The script engines dropped while this thread was inside a call into it, by language: told when the outermost
    /// call returns ([`Entered`]'s drop), so their realms go too.
    dropped: Mutex<Vec<(usize, u64)>>,
}

/// How many times the script engines dropped during a call are told in a row: telling one may drop more.
const MAX_DROP_ROUNDS: usize = 64;

/// About the stack one script level takes (measured: 2.5 KiB in a debug build, 0.65 KiB in a release one): the rate the
/// stack an extension uses between a call into it and its call back is charged to the script engine's call depth at.
const DEPTH_UNIT_BYTES: usize = if cfg!(debug_assertions) { 2560 } else { 640 };

/// The call-depth units the engine's own frames around a call into an extension and its call back cost, on top of what
/// the extension used (the native's dispatch, the call's frame, the call back's run).
const CALL_BACK_COST: usize = 4;

/// A number for the calling thread, never 0 and different for every live thread (what `ExtensionState::owner` holds).
fn thread_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static TOKEN: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TOKEN.with(|token| *token)
}

/// Where this thread's stack is now: the address of a local (the stack grows down on every platform the engine runs on).
#[inline(never)]
fn stack_mark() -> usize {
    let here = 0u8;
    std::hint::black_box(&here) as *const u8 as usize
}

/// Why a call could not go into an extension.
pub(crate) enum Refused {
    /// The thread is already inside the extension, which is not calling the engine back.
    Reentered,
    /// The calls back nest deeper than [`TORQUE_MAX_NESTING`].
    TooDeep,
}

impl ExtensionState {
    fn new(
        registered: Registered,
        sha256: Option<String>,
        library: Option<PathBuf>,
        loaded_from: Option<PathBuf>,
    ) -> ExtensionState {
        ExtensionState {
            id: registered.id,
            version: registered.version,
            sha256,
            library,
            loaded_from,
            net_version: registered.net_version,
            natives: registered.natives,
            globals: registered.globals,
            passes: registered.passes,
            languages: registered.languages,
            disabled: Mutex::new(None),
            off: AtomicBool::new(false),
            calls: Mutex::new(()),
            owner: AtomicU64::new(0),
            nesting: AtomicUsize::new(0),
            calling_out: AtomicUsize::new(0),
            marks: Mutex::new(Vec::new()),
            fallback_steps: AtomicU64::new(0),
            log: Mutex::new(registered.log),
            dropped: Mutex::new(Vec::new()),
        }
    }

    /// Why it is disabled, or `None`.
    pub(crate) fn disabled(&self) -> Option<String> {
        if !self.is_off() {
            return None;
        }
        self.disabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether it is disabled (the check a call makes: no lock, no copy of the reason).
    pub(crate) fn is_off(&self) -> bool {
        self.off.load(Ordering::Acquire)
    }

    /// Disables it: no call goes into it any more.
    pub(crate) fn disable(&self, reason: String) {
        self.note(format!("error: disabled: {reason}"));
        self.disabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
        self.off.store(true, Ordering::Release);
    }

    /// Keeps a line of its log.
    pub(crate) fn note(&self, line: String) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        if log.len() >= MAX_LOG_LINES {
            log.remove(0);
        }
        log.push(line);
    }

    /// The lock held for a call into it: one call at a time, from any thread. `None` when this thread is already in a
    /// call into it (the call would re-enter the extension, which waits for nothing but itself).
    pub(crate) fn enter(&self) -> Option<Entered<'_>> {
        let me = thread_token();
        if self.owner.load(Ordering::Acquire) == me {
            return None;
        }
        Some(self.enter_outer(me))
    }

    /// The lock held for a script's call into it: one call at a time, from any thread, as [`enter`](Self::enter); and on
    /// the thread already inside it, a nested call when every live call is calling the engine back (a script it ran
    /// calls it again), at most [`TORQUE_MAX_NESTING`] deep.
    pub(crate) fn enter_call(&self) -> Result<Entered<'_>, Refused> {
        let me = thread_token();
        if self.owner.load(Ordering::Acquire) != me {
            return Ok(self.enter_outer(me));
        }
        let live = self.nesting.load(Ordering::SeqCst);
        if self.calling_out.load(Ordering::SeqCst) < live {
            return Err(Refused::Reentered);
        }
        if live >= TORQUE_MAX_NESTING {
            return Err(Refused::TooDeep);
        }
        self.nesting.fetch_add(1, Ordering::SeqCst);
        self.lock_marks().push(stack_mark());
        Ok(Entered {
            state: self,
            lock: None,
        })
    }

    fn lock_marks(&self) -> MutexGuard<'_, Vec<usize>> {
        self.marks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The call-depth units a call back from the innermost live call costs: the stack the extension used since that
    /// call came in, in script levels' worth ([`DEPTH_UNIT_BYTES`]), so that recursion through the extension stops with
    /// `CallDepth` before the end of the stack as a script's does.
    pub(crate) fn call_back_depth(&self) -> usize {
        let entry = self.lock_marks().last().copied().unwrap_or(0);
        entry
            .saturating_sub(stack_mark())
            .div_ceil(DEPTH_UNIT_BYTES)
            .saturating_add(CALL_BACK_COST)
    }

    fn enter_outer(&self, me: u64) -> Entered<'_> {
        let lock = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        self.owner.store(me, Ordering::Release);
        self.nesting.store(1, Ordering::SeqCst);
        self.calling_out.store(0, Ordering::SeqCst);
        self.fallback_steps.store(0, Ordering::SeqCst);
        {
            let mut marks = self.lock_marks();
            marks.clear();
            marks.push(stack_mark());
        }
        Entered {
            state: self,
            lock: Some(lock),
        }
    }

    /// Runs `f`, the engine's work for a call the extension made back into it (a script function it runs): a call into
    /// the extension that `f` makes is a nested one ([`enter_call`](Self::enter_call)).
    pub(crate) fn call_out<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Back<'a>(&'a AtomicUsize);
        impl Drop for Back<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.calling_out.fetch_add(1, Ordering::SeqCst);
        let _back = Back(&self.calling_out);
        f()
    }

    /// The error of a call refused by [`enter_call`](Self::enter_call).
    fn refusal(&self, refused: Refused) -> ScriptError {
        match refused {
            Refused::Reentered => ScriptError::new(format!(
                "the native extension {} was called again from inside one of its own calls",
                self.id
            )),
            Refused::TooDeep => ScriptError::new(format!(
                "the native extension {}: calls back into it nest deeper than {TORQUE_MAX_NESTING}",
                self.id
            )),
        }
    }

    /// Runs the file `path` of its language `lang` in `vm` (`exec` found it): what the language's
    /// [`ScriptLanguage`](torque_ts_vm::policy::ScriptLanguage) does.
    pub(crate) fn run_file(
        self: &Arc<Self>,
        lang: usize,
        vm: &mut Vm,
        path: &[u8],
        source: &[u8],
    ) -> Result<(), VmError> {
        if let Some(reason) = self.disabled() {
            vm.error(format!(
                "the native extension {} is disabled ({reason}): {} did not run",
                self.id,
                torque_ts_vm::vm::lossy(path)
            ));
            return Ok(());
        }
        let package = vm
            .namespaces()
            .declare_package_for(Some(path))
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        if !package.is_empty() {
            VmContext::of(vm).allow_package(&package);
        }
        let outcome = {
            let entered = match self.enter_call() {
                Ok(entered) => entered,
                Err(Refused::TooDeep) => return Err(VmError::CallDepth),
                Err(refused) => {
                    vm.error(self.refusal(refused).to_string());
                    return Ok(());
                }
            };
            if self.is_off() {
                return Ok(());
            }
            let outcome = ffi::run_file(self, lang, vm, path, source, package);
            drop(entered);
            outcome
        };
        match outcome {
            CallOutcome::Done(_) => Ok(()),
            CallOutcome::Failed(message) => {
                vm.error(message);
                Ok(())
            }
            CallOutcome::Limit(error) => Err(error),
            CallOutcome::Panicked => {
                self.disable(format!(
                    "it panicked running {}",
                    torque_ts_vm::vm::lossy(path)
                ));
                vm.error(format!(
                    "the native extension {} panicked and is disabled",
                    self.id
                ));
                Ok(())
            }
        }
    }

    /// Calls the function its language `lang` defined with `id`, from the script engine `vm`.
    pub(crate) fn call_defined(
        self: &Arc<Self>,
        lang: usize,
        id: u64,
        vm: &mut Vm,
        call: &NativeCall,
    ) -> Native {
        if let Some(reason) = self.disabled() {
            vm.error(format!(
                "the native extension {} is disabled ({reason})",
                self.id
            ));
            return Native::Void;
        }
        let args = call.argv.get(1..).unwrap_or(&[]);
        let outcome = {
            let entered = match self.enter_call() {
                Ok(entered) => entered,
                Err(Refused::TooDeep) => return Native::Fail(VmError::CallDepth),
                Err(refused) => {
                    vm.error(self.refusal(refused).to_string());
                    return Native::Void;
                }
            };
            if self.is_off() {
                return Native::Void;
            }
            let outcome = ffi::call_defined(self, lang, id, vm, args, call.this_object);
            drop(entered);
            outcome
        };
        match outcome {
            CallOutcome::Done(native) => native,
            CallOutcome::Failed(message) => {
                vm.error(message);
                Native::Void
            }
            CallOutcome::Limit(error) => Native::Fail(error),
            CallOutcome::Panicked => {
                self.disable(format!(
                    "it panicked in a function its language defined ({id})"
                ));
                vm.error(format!(
                    "the native extension {} panicked and is disabled",
                    self.id
                ));
                Native::Void
            }
        }
    }

    /// Tells its language `lang` that the script engine `vm` is gone.
    pub(crate) fn vm_dropped(&self, lang: usize, vm: u64) {
        if self.is_off() {
            return;
        }
        // From the engine's own work, never inside a call of the extension's on this thread (a script engine is dropped
        // by its host). A drop inside a call (a script the call ran dropped another script engine) waits for the
        // outermost call to return rather than re-entering.
        match self.enter() {
            Some(_one_at_a_time) => ffi::vm_dropped(&self.languages[lang], vm),
            None => self
                .dropped
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((lang, vm)),
        }
    }

    /// Tells the languages about the script engines dropped during the call that is returning, under its lock.
    fn tell_dropped(&self) {
        for _ in 0..MAX_DROP_ROUNDS {
            let dropped =
                std::mem::take(&mut *self.dropped.lock().unwrap_or_else(PoisonError::into_inner));
            if dropped.is_empty() || self.is_off() {
                return;
            }
            for (lang, vm) in dropped {
                ffi::vm_dropped(&self.languages[lang], vm);
            }
        }
    }

    /// Calls its native `index` from the script engine `vm`.
    fn call_native(
        self: &Arc<Self>,
        index: usize,
        vm: &mut Vm,
        call: &NativeCall,
        first: usize,
    ) -> Result<Native, ScriptError> {
        if let Some(reason) = self.disabled() {
            return Err(ScriptError::new(format!(
                "the native extension {} is disabled ({reason})",
                self.id
            )));
        }
        let spec = &self.natives[index];
        // The method's object, as the engine's own `This` reads it.
        let this = if spec.method { call.this_object } else { None };
        let args = call.argv.get(first..).unwrap_or(&[]);
        let outcome = {
            let entered = match self.enter_call() {
                Ok(entered) => entered,
                Err(Refused::TooDeep) => return Ok(Native::Fail(VmError::CallDepth)),
                Err(refused) => return Err(self.refusal(refused)),
            };
            // Disabled by another thread's call while this one waited for the lock: not called again.
            if let Some(reason) = self.disabled() {
                return Err(ScriptError::new(format!(
                    "the native extension {} is disabled ({reason})",
                    self.id
                )));
            }
            let outcome = ffi::call_native(self, spec, vm, args, this);
            drop(entered);
            outcome
        };
        match outcome {
            CallOutcome::Done(native) => Ok(native),
            CallOutcome::Failed(message) => Err(ScriptError::new(message)),
            CallOutcome::Limit(error) => Ok(Native::Fail(error)),
            CallOutcome::Panicked => {
                let label = if spec.namespace.is_empty() {
                    spec.name.clone()
                } else {
                    format!("{}::{}", spec.namespace, spec.name)
                };
                self.disable(format!("it panicked in {label}"));
                Err(ScriptError::new(format!(
                    "the native extension {} panicked and is disabled",
                    self.id
                )))
            }
        }
    }
}

/// A call into an extension in progress: [`ExtensionState::enter`]'s lock, or a nested call's place under it.
pub(crate) struct Entered<'a> {
    state: &'a ExtensionState,
    /// The lock, for the outermost call; none for a nested one.
    lock: Option<MutexGuard<'a, ()>>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.state.lock_marks().pop();
        if self.lock.is_none() {
            self.state.nesting.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        // The outermost call has returned: the script engines dropped meanwhile are told now, still one call at a time
        // (not while a panic unwinds: they wait for the next call).
        if !std::thread::panicking() {
            self.state.tell_dropped();
        }
        self.state.nesting.store(0, Ordering::SeqCst);
        // Before the lock is let go (fields drop after this).
        self.state.owner.store(0, Ordering::Release);
    }
}

/// A loaded native extension, as hosts and tests see it: its id, version and hash, whether it is disabled, its log. The
/// engine lists one for each extension it built (`engine.list::<Extension>()`).
#[derive(Clone)]
pub struct Extension(pub(crate) Arc<ExtensionState>);

impl Extension {
    /// Its id (its plugin id).
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// Its version.
    pub fn version(&self) -> &str {
        &self.0.version
    }

    /// Its library's SHA-256 hash; `None` for one linked into the game.
    pub fn sha256(&self) -> Option<&str> {
        self.0.sha256.as_deref()
    }

    /// Its library's path in the plugins folder; `None` for one linked into the game.
    pub fn library(&self) -> Option<&Path> {
        self.0.library.as_deref()
    }

    /// The private copy it was loaded from (`<sha256>.<ext>` in the per-user cache); `None` for one linked into the
    /// game.
    pub fn loaded_from(&self) -> Option<&Path> {
        self.0.loaded_from.as_deref()
    }

    /// Its network version, when it has one.
    pub fn net_version(&self) -> Option<u32> {
        self.0.net_version
    }

    /// Whether it is disabled (after a panic): its functions then print an error and return the empty string, and its
    /// passes draw nothing.
    pub fn is_disabled(&self) -> bool {
        self.0.disabled().is_some()
    }

    /// Why it is disabled.
    pub fn disabled_reason(&self) -> Option<String> {
        self.0.disabled()
    }

    /// What it wrote to the engine's log outside a script call (its init, its passes), and why it was disabled.
    pub fn log(&self) -> Vec<String> {
        self.0
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extension")
            .field("id", &self.0.id)
            .field("version", &self.0.version)
            .field("disabled", &self.0.disabled())
            .finish_non_exhaustive()
    }
}

/// The plugin of one loaded extension: its id is the one the extension reports, and its `build` adds the console
/// functions, methods, globals and passes the extension registered, as a compiled plugin's would.
///
/// [`NativeExtensionsPlugin`](crate::NativeExtensionsPlugin) makes one for each library it loads; [`from_init`](Self::from_init)
/// makes one for an extension linked into the game (its entry point called directly, with no library and no approval).
pub struct ExtensionPlugin(pub(crate) Arc<ExtensionState>);

impl PluginType for ExtensionPlugin {
    /// The adapter's own id, never an engine's: each extension's plugin is known by the id it reports.
    const ID: PluginId = PluginId::new("torque.native-extension");
}

impl ExtensionPlugin {
    /// The extension whose entry point is `init`, linked into the game (`torque_plugin_sdk::init_fn!`), built for the
    /// ABI version `abi_version`: Err with the reason when that is not this engine's, or its init fails or declares
    /// something the engine refuses.
    pub fn from_init(
        abi_version: u32,
        init: TorquePluginInitFn,
    ) -> Result<ExtensionPlugin, String> {
        ffi::check_abi(abi_version)?;
        let registered = ffi::run_init(init)?;
        Ok(ExtensionPlugin(Arc::new(ExtensionState::new(
            registered, None, None, None,
        ))))
    }

    /// The extension of the library `original`, whose manifest is `manifest` and whose hash `sha256` was approved,
    /// loaded from its private copy `copy` (an absolute path).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn load_library(
        held: &crate::stage::Held,
        original: &Path,
        manifest: &Manifest,
        sha256: &str,
    ) -> Result<ExtensionPlugin, String> {
        let copy = held.path.as_path();
        let registered = ffi::load_library(&held.open_path(), &|| held.still_same())?;
        if registered.id != manifest.id || registered.version != manifest.version {
            return Err(format!(
                "it reports {} {}, and its manifest says {} {}",
                registered.id, registered.version, manifest.id, manifest.version
            ));
        }
        Ok(ExtensionPlugin(Arc::new(ExtensionState::new(
            registered,
            Some(sha256.to_string()),
            Some(original.to_path_buf()),
            Some(copy.to_path_buf()),
        ))))
    }

    /// In a browser build no library loads.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn load_library(
        _held: &crate::stage::Held,
        _original: &Path,
        _manifest: &Manifest,
        _sha256: &str,
    ) -> Result<ExtensionPlugin, String> {
        Err("native extensions do not load in a browser build".to_string())
    }

    /// The extension, as hosts and tests see it.
    pub fn extension(&self) -> Extension {
        Extension(Arc::clone(&self.0))
    }
}

impl Plugin for ExtensionPlugin {
    fn id(&self) -> Option<&str> {
        Some(&self.0.id)
    }

    fn net_version(&self) -> Option<u32> {
        self.0.net_version
    }

    fn build(&self, engine: &mut EngineBuilder) {
        let state = &self.0;
        for (index, spec) in state.natives.iter().enumerate() {
            let kinds: Vec<_> = spec.args.iter().map(|(_, kind)| *kind).collect();
            let shared = Arc::clone(state);
            let native = match TypedNative::from_call(&kinds, move |vm, call, first| {
                shared.call_native(index, vm, call, first)
            }) {
                Ok(native) => native,
                Err(message) => {
                    engine.reject(format!(
                        "native extension {}: {}: {message}",
                        state.id, spec.name
                    ));
                    continue;
                }
            };
            let callee = if spec.method {
                Callee::Method {
                    class: &spec.namespace,
                    name: &spec.name,
                }
            } else {
                Callee::Function {
                    namespace: &spec.namespace,
                    name: &spec.name,
                }
            };
            let names: Vec<&str> = spec.args.iter().map(|(name, _)| name.as_str()).collect();
            // The docs live for the process, as the extension does (once per extension and native).
            let docs: &'static str = if spec.docs.is_empty() {
                ""
            } else {
                Box::leak(spec.docs.clone().into_boxed_str())
            };
            registry(engine, spec.side).with_effects(spec.effects, |r| {
                r.native_at_run_time(
                    callee,
                    &names,
                    &spec.description,
                    spec.usage.as_deref(),
                    spec.dev,
                    docs,
                    native,
                );
            });
        }
        for global in &state.globals {
            registry(engine, global.side).global(&global.name, &global.default);
        }
        for (lang, spec) in state.languages.iter().enumerate() {
            add_language(engine, state, lang, spec.side);
        }
        if !state.passes.is_empty() {
            #[cfg(feature = "render")]
            crate::render::add_passes(engine, state);
            #[cfg(not(feature = "render"))]
            engine.warn(format!(
                "native extension {}: its {} render pass(es) are skipped: this build of the loader has no `render` feature",
                state.id,
                state.passes.len()
            ));
        }
        engine.push(Extension(Arc::clone(state)));
    }
}

/// Installs the extension's language `lang` in every script engine of `side`, as early as each is made (its classes'
/// stage, then again at the next stage for a host that skips it).
fn add_language(engine: &mut EngineBuilder, state: &Arc<ExtensionState>, lang: usize, side: Side) {
    let install = |state: &Arc<ExtensionState>, side: Side| {
        let state = Arc::clone(state);
        move |vm: &mut Vm, _: &torque_engine::Engine| {
            LanguageRunner::install(vm, &state, lang, side);
            Ok(())
        }
    };
    if matches!(side, Side::Both | Side::Server) {
        engine.server(ServerStage::Classes, install(state, Side::Server));
        engine.server(ServerStage::Prepare, install(state, Side::Server));
    }
    if matches!(side, Side::Both | Side::Client) {
        engine.client(ClientStage::Classes, install(state, Side::Client));
        engine.client(ClientStage::Game, install(state, Side::Client));
    }
}

fn registry(engine: &mut EngineBuilder, side: Side) -> torque_engine::ScriptRegistry<'_> {
    match side {
        Side::Both => engine.script(),
        Side::Server => engine.server_script(),
        Side::Client => engine.client_script(),
    }
}
