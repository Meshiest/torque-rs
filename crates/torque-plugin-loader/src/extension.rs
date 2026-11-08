//! A loaded extension in the engine: its state (disabled after a panic), the plugin that adds what it registered, and the
//! handle hosts and tests read it through.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use torque_engine::script::{Callee, TypedNative};
use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType, Side};
use torque_plugin_abi::TorquePluginInitFn;
use torque_ts_vm::convert::ScriptError;
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use crate::ffi::{self, CallOutcome, Registered};
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
    /// Why it is disabled, once it is.
    disabled: Mutex<Option<String>>,
    /// Held for each call into it: one at a time.
    calls: Mutex<()>,
    /// The thread holding `calls`.
    owner: Mutex<Option<std::thread::ThreadId>>,
    log: Mutex<Vec<String>>,
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
            disabled: Mutex::new(None),
            calls: Mutex::new(()),
            owner: Mutex::new(None),
            log: Mutex::new(registered.log),
        }
    }

    /// Why it is disabled, or `None`.
    pub(crate) fn disabled(&self) -> Option<String> {
        self.disabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Disables it: no call goes into it any more.
    pub(crate) fn disable(&self, reason: String) {
        self.note(format!("error: disabled: {reason}"));
        self.disabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
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
        let me = std::thread::current().id();
        if *self.owner.lock().unwrap_or_else(PoisonError::into_inner) == Some(me) {
            return None;
        }
        let lock = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        *self.owner.lock().unwrap_or_else(PoisonError::into_inner) = Some(me);
        Some(Entered {
            state: self,
            _lock: lock,
        })
    }

    /// Calls its native `index` from the script engine `vm`.
    fn call_native(
        &self,
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
            let Some(_one_at_a_time) = self.enter() else {
                return Err(ScriptError::new(format!(
                    "the native extension {} was called again from inside one of its own calls",
                    self.id
                )));
            };
            // Disabled by another thread's call while this one waited for the lock: not called again.
            if let Some(reason) = self.disabled() {
                return Err(ScriptError::new(format!(
                    "the native extension {} is disabled ({reason})",
                    self.id
                )));
            }
            ffi::call_native(spec, vm, args, this)
        };
        match outcome {
            CallOutcome::Done(native) => Ok(native),
            CallOutcome::Failed(message) => Err(ScriptError::new(message)),
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

/// A call into an extension in progress: [`ExtensionState::enter`]'s lock.
pub(crate) struct Entered<'a> {
    state: &'a ExtensionState,
    _lock: MutexGuard<'a, ()>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        // Before the lock is let go (fields drop after this).
        *self
            .state
            .owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
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
        copy: &Path,
        original: &Path,
        manifest: &Manifest,
        sha256: &str,
    ) -> Result<ExtensionPlugin, String> {
        let registered = ffi::load_library(copy)?;
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
        _copy: &Path,
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
            registry(engine, spec.side).native_at_run_time(
                callee,
                &names,
                &spec.description,
                spec.usage.as_deref(),
                spec.dev,
                docs,
                native,
            );
        }
        for global in &state.globals {
            registry(engine, global.side).global(&global.name, &global.default);
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

fn registry(engine: &mut EngineBuilder, side: Side) -> torque_engine::ScriptRegistry<'_> {
    match side {
        Side::Both => engine.script(),
        Side::Server => engine.server_script(),
        Side::Client => engine.client_script(),
    }
}
