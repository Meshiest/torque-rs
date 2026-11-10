//! Lua in a browser build: the interface the page's Luau module plugs into, and the stand-in for a page without one.
//!
//! Native extensions do not load in a browser, so a page's Lua is a second WebAssembly module: the Lua extension itself
//! (`crates/torque-plugin-lua`, the same Luau and the same sandbox) built for `wasm32-wasip1` (`just lua-web`), loaded by
//! the page (`web/lua.js`) and bridged through its JavaScript. [`WebLuaPlugin`] installs the `.lua` and `.luau` languages
//! in every script engine and hands their files to a [`LuaModule`]; the module calls the script engine back through the
//! [`LuaHost`] it is handed, which offers what the C ABI's script functions offer a native extension, with the same checks
//! (the names it takes, the call policy, `exec`'s checks for every file it reads, the packages its definitions may go
//! into, the step budget, how deep calls into it nest). It is the one place those checks are made for the page's Lua.
//!
//! In a browser build the plugin's default module is `PageLuaModule`, the bridge to the page's module; a page that did not
//! load one (a game without Lua) gets what [`MissingLuaModule`] does: a Lua file prints that it cannot run. Natively the
//! default is [`MissingLuaModule`], and the plugin's host is tested with a module written in Rust.

use std::cell::Cell;
use std::rc::Rc;

use torque_engine::{ClientStage, EngineBuilder, Plugin, ServerStage, Side};
use torque_plugin_abi::{
    TORQUE_DEFAULT_STEPS, TORQUE_MAX_ARGS, TORQUE_MAX_NAME, TORQUE_MAX_NESTING, TORQUE_MAX_SCRIPT,
    TORQUE_MAX_TEXT, TORQUE_MAX_VALUE,
};
use torque_ts_vm::ns::Callable;
use torque_ts_vm::policy::ScriptLanguage;
use torque_ts_vm::vm::{Level, MAX_CALL_DEPTH, Native, Vm, VmError, WORK_PER_STEP};

use crate::language::{VmContext, is_global_name, is_plain_path, is_script_name, read_checked};

#[cfg(target_arch = "wasm32")]
mod page;
#[cfg(target_arch = "wasm32")]
pub use page::PageLuaModule;

/// The call-depth units a call back from the module costs: its frames are the page's (the module's and the bridge's),
/// out of the engine's sight, so a fixed share of the budget stands in for them.
const CALL_BACK_COST: usize = 8;

/// The global holding each realm's memory cap, in MiB: the Lua extension's one setting, given its default (64) in every
/// script engine that has none.
pub const MEMORY_LIMIT_PREF: &str = "$Pref::Lua::MemoryLimit";

/// The interpreter of a page's Lua: what the bridge to the separately loaded module answers.
pub trait LuaModule {
    /// Runs the file `path` (its text `source`) in the realm of `host`'s script engine. `Err` with the error that stopped
    /// it (a limit: the run's step budget, the call depth); a script error is printed and is `Ok`.
    fn run_file(&self, host: &mut dyn LuaHost, path: &str, source: &[u8]) -> Result<(), VmError>;

    /// Calls the function the module defined with `id` ([`LuaHost::define_function`]) with `args`; its result is text.
    fn call_defined(
        &self,
        host: &mut dyn LuaHost,
        id: u64,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError>;

    /// The script engine whose realm is `realm` is gone.
    fn realm_dropped(&self, realm: u64) {
        let _ = realm;
    }
}

/// Why the engine did not do what a module asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// It refused: a name that is not one, too many or too long arguments, or the call policy (which printed why).
    Refused,
    /// A limit stopped the run (the step budget, the call depth): stop, and hand it back from the module's own call so
    /// that the script that started the run stops too.
    Stopped(VmError),
}

/// What the engine offers a [`LuaModule`] during one of its calls: the C ABI's script functions, as Rust, with the checks
/// the C ABI makes.
pub trait LuaHost {
    /// The realm's number (the script engine's, never another's) and side.
    fn realm(&self) -> (u64, Side);
    /// During a `run_file`: the package the file's definitions go into (a mod's), or the empty text.
    fn package(&self) -> &str;
    /// Runs the console function `namespace::name` and gives its result (the empty text for a function that does not
    /// exist).
    fn call_function(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, HostError>;
    /// Runs the method `name` of `object`, as [`call_function`](Self::call_function).
    fn call_method(
        &mut self,
        object: u32,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, HostError>;
    /// Whether `object` exists and has a method `name`.
    fn has_method(&self, object: u32, name: &str) -> bool;
    /// The object `name` names.
    fn find_object(&self, name: &[u8]) -> Option<u32>;
    /// The field `name[index]` of `object`; `None` when the object does not exist.
    fn field(&self, object: u32, name: &[u8], index: &[u8]) -> Option<Vec<u8>>;
    /// Sets the field `name[index]` of `object`: false when the object does not exist or the engine refused it.
    fn set_field(&mut self, object: u32, name: &[u8], index: &[u8], value: &[u8]) -> bool;
    /// A global's value (`name` with its `$`): empty when it is not set or `name` is not a global's name.
    fn global(&self, name: &str) -> Vec<u8>;
    /// Sets a global: false when `name` is not a global's name or the value is too long.
    fn set_global(&mut self, name: &str, value: &[u8]) -> bool;
    /// Defines a console function whose calls come to [`LuaModule::call_defined`] with `id`, its usage text `usage`:
    /// false when the names or the package are refused.
    fn define_function(
        &mut self,
        namespace: &str,
        name: &str,
        package: &str,
        usage: &str,
        id: u64,
    ) -> bool;
    /// Charges `steps` of work: `Err(StepLimit)` once the budget is spent.
    fn charge(&mut self, steps: u64) -> Result<(), VmError>;
    /// The Lua file `path` after `exec`'s checks; `None` when it is refused or missing.
    fn read_script(&mut self, path: &str) -> Result<Option<Vec<u8>>, VmError>;
    /// Prints on the console.
    fn print(&mut self, level: Level, text: &str);
}

/// The stand-in for a page that has no Lua module: every Lua file prints that it cannot run.
#[derive(Clone, Copy, Debug, Default)]
pub struct MissingLuaModule;

impl LuaModule for MissingLuaModule {
    fn run_file(&self, host: &mut dyn LuaHost, path: &str, _source: &[u8]) -> Result<(), VmError> {
        host.print(
            Level::Error,
            &format!("{path} did not run: Lua needs the page's Lua module, which this build does not have"),
        );
        Ok(())
    }

    fn call_defined(&self, _: &mut dyn LuaHost, _: u64, _: &[Vec<u8>]) -> Result<Vec<u8>, VmError> {
        Ok(Vec::new())
    }
}

/// Lua through a [`LuaModule`]: the browser build's counterpart of the native Lua extension. Its id is `torque.lua`, the
/// extension's, so a mod's `requires_plugins = ["torque.lua"]` holds for both.
#[derive(torque_engine::PluginId)]
#[plugin(id = "torque.lua")]
pub struct WebLuaPlugin {
    module: Rc<dyn LuaModule>,
}

impl Default for WebLuaPlugin {
    /// The page's module in a browser build (`PageLuaModule`), else [`MissingLuaModule`].
    fn default() -> WebLuaPlugin {
        #[cfg(target_arch = "wasm32")]
        return WebLuaPlugin::new(PageLuaModule);
        #[cfg(not(target_arch = "wasm32"))]
        WebLuaPlugin::new(MissingLuaModule)
    }
}

impl WebLuaPlugin {
    /// Lua run by `module`.
    pub fn new(module: impl LuaModule + 'static) -> WebLuaPlugin {
        WebLuaPlugin {
            module: Rc::new(module),
        }
    }
}

impl Plugin for WebLuaPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let install = |module: &Rc<dyn LuaModule>, side: Side| {
            let module = Rc::clone(module);
            move |vm: &mut Vm, _: &torque_engine::Engine| {
                Runner::install(vm, &module, side);
                Ok(())
            }
        };
        engine.server(ServerStage::Classes, install(&self.module, Side::Server));
        engine.server(ServerStage::Prepare, install(&self.module, Side::Server));
        engine.client(ClientStage::Classes, install(&self.module, Side::Client));
        engine.client(ClientStage::Game, install(&self.module, Side::Client));
    }
}

/// The `.lua` and `.luau` languages of one script engine.
struct Runner {
    module: Rc<dyn LuaModule>,
    /// The steps the outermost call charged when the script engine has no step limit.
    fallback: Rc<Cell<u64>>,
    /// How deep calls into the module nest now.
    nesting: Rc<Cell<usize>>,
}

/// Tells the module its realm went, when the script engine goes.
struct Dropped {
    module: Rc<dyn LuaModule>,
    realm: u64,
}

impl Drop for Dropped {
    fn drop(&mut self) {
        self.module.realm_dropped(self.realm);
    }
}

impl Runner {
    fn install(vm: &mut Vm, module: &Rc<dyn LuaModule>, side: Side) {
        if vm.extension::<Dropped>().is_some() {
            return;
        }
        let context = VmContext::of(vm);
        context.set_side(side);
        if vm.global(MEMORY_LIMIT_PREF).is_none_or(|v| v.is_empty()) {
            vm.set_global(MEMORY_LIMIT_PREF, "64");
        }
        vm.set_extension(Rc::new(Dropped {
            module: Rc::clone(module),
            realm: context.id,
        }));
        let runner = Rc::new(Runner {
            module: Rc::clone(module),
            fallback: Rc::new(Cell::new(0)),
            nesting: Rc::new(Cell::new(0)),
        });
        for extension in ["lua", "luau"] {
            vm.set_script_language(extension, Rc::clone(&runner) as Rc<dyn ScriptLanguage>);
        }
    }

    fn host<'a>(&self, vm: &'a mut Vm, package: String) -> VmHost<'a> {
        VmHost {
            vm,
            module: Rc::clone(&self.module),
            fallback: Rc::clone(&self.fallback),
            nesting: Rc::clone(&self.nesting),
            package,
        }
    }

    /// Runs `f` as one call into the module: the outermost starts the fallback budget again, and a call nested more than
    /// [`TORQUE_MAX_NESTING`] deep is refused with [`VmError::CallDepth`], as a native extension's is.
    fn enter<T>(&self, f: impl FnOnce() -> Result<T, VmError>) -> Result<T, VmError> {
        let nesting = self.nesting.get();
        if nesting >= TORQUE_MAX_NESTING {
            return Err(VmError::CallDepth);
        }
        if nesting == 0 {
            self.fallback.set(0);
        }
        self.nesting.set(nesting + 1);
        let out = f();
        self.nesting.set(nesting);
        out
    }
}

impl ScriptLanguage for Runner {
    fn run(&self, vm: &mut Vm, path: &[u8], source: &[u8]) -> Result<(), VmError> {
        let package = vm
            .namespaces()
            .declare_package_for(Some(path))
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .unwrap_or_default();
        if !package.is_empty() {
            VmContext::of(vm).allow_package(package.as_bytes());
        }
        let path = String::from_utf8_lossy(path).into_owned();
        let module = Rc::clone(&self.module);
        let mut host = self.host(vm, package);
        self.enter(|| module.run_file(&mut host, &path, source))
    }
}

/// The [`LuaHost`] over one script engine.
struct VmHost<'a> {
    vm: &'a mut Vm,
    module: Rc<dyn LuaModule>,
    fallback: Rc<Cell<u64>>,
    nesting: Rc<Cell<usize>>,
    package: String,
}

impl VmHost<'_> {
    /// Runs script for the module: charged [`CALL_BACK_COST`] call-depth units, and stopped past the limit.
    fn back(
        &mut self,
        run: impl FnOnce(&mut Vm) -> Result<Vec<u8>, VmError>,
    ) -> Result<Vec<u8>, HostError> {
        if self.vm.call_depth().saturating_add(CALL_BACK_COST) > MAX_CALL_DEPTH {
            return Err(HostError::Stopped(VmError::CallDepth));
        }
        self.vm
            .with_extra_depth(CALL_BACK_COST, run)
            .map_err(HostError::Stopped)
    }

    /// Whether the call policy lets the module run `namespace::name` with `args`: the reason is printed when it does not.
    fn allowed(&mut self, namespace: &str, name: &str, method: bool, args: &[Vec<u8>]) -> bool {
        let kind = crate::call_policy::CallKind::of(method);
        match crate::call_policy::check(self.vm, namespace, name, kind, args) {
            Ok(()) => true,
            Err(reason) => {
                self.vm.error(reason);
                false
            }
        }
    }
}

impl LuaHost for VmHost<'_> {
    fn realm(&self) -> (u64, Side) {
        match self.vm.extension::<VmContext>() {
            Some(context) => (context.id, context.side()),
            None => (0, Side::Both),
        }
    }

    fn package(&self) -> &str {
        &self.package
    }

    fn call_function(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, HostError> {
        if !is_name(name) || !(namespace.is_empty() || is_name(namespace)) || !are_args(args) {
            return Err(HostError::Refused);
        }
        if !self.allowed(namespace, name, false, args) {
            return Err(HostError::Refused);
        }
        let namespace = (!namespace.is_empty()).then_some(namespace.as_bytes());
        self.back(|vm| {
            let result = vm.invoke(namespace, name.as_bytes(), args)?;
            crate::call_policy::note_deferred(vm, name, &result);
            Ok(result)
        })
    }

    fn call_method(
        &mut self,
        object: u32,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, HostError> {
        if !is_name(name) || !are_args(args) {
            return Err(HostError::Refused);
        }
        if !self.allowed("", name, true, args) {
            return Err(HostError::Refused);
        }
        self.back(|vm| {
            let result = vm.call_method(object, name.as_bytes(), args)?;
            crate::call_policy::note_deferred(vm, name, &result);
            Ok(result)
        })
    }

    fn has_method(&self, object: u32, name: &str) -> bool {
        name.len() <= TORQUE_MAX_NAME
            && self.vm.objects().get(object).is_some()
            && self.vm.method_origin(object, name.as_bytes()).is_some()
    }

    fn find_object(&self, name: &[u8]) -> Option<u32> {
        if name.len() > TORQUE_MAX_NAME {
            return None;
        }
        self.vm.objects().find(name)
    }

    fn field(&self, object: u32, name: &[u8], index: &[u8]) -> Option<Vec<u8>> {
        if name.len() > TORQUE_MAX_NAME || index.len() > TORQUE_MAX_NAME {
            return None;
        }
        self.vm.objects().get(object)?;
        Some(self.vm.script_field(object, name, index))
    }

    fn set_field(&mut self, object: u32, name: &[u8], index: &[u8], value: &[u8]) -> bool {
        let fits = name.len() <= TORQUE_MAX_NAME
            && index.len() <= TORQUE_MAX_NAME
            && value.len() <= TORQUE_MAX_VALUE;
        if name.is_empty() || !fits || self.vm.objects().get(object).is_none() {
            return false;
        }
        if let Err(reason) = crate::call_policy::check_field(self.vm, name, index, value) {
            self.vm.error(reason);
            return false;
        }
        self.vm.script_set_field(object, name, index, value);
        true
    }

    fn global(&self, name: &str) -> Vec<u8> {
        if name.len() > TORQUE_MAX_NAME || !is_global_name(name) {
            return Vec::new();
        }
        self.vm.global(name).unwrap_or_default()
    }

    fn set_global(&mut self, name: &str, value: &[u8]) -> bool {
        if name.len() > TORQUE_MAX_NAME || !is_global_name(name) || value.len() > TORQUE_MAX_VALUE {
            return false;
        }
        if let Err(reason) = crate::call_policy::check_global(self.vm, name, value) {
            self.vm.error(reason);
            return false;
        }
        self.vm.set_global_text(name.as_bytes(), value);
        true
    }

    fn define_function(
        &mut self,
        namespace: &str,
        name: &str,
        package: &str,
        usage: &str,
        id: u64,
    ) -> bool {
        if !is_name(name)
            || !(namespace.is_empty() || is_name(namespace))
            || !(package.is_empty() || is_name(package))
            || usage.len() > TORQUE_MAX_TEXT
        {
            return false;
        }
        if !package.is_empty() && !VmContext::of(self.vm).package_allowed(package.as_bytes()) {
            return false;
        }
        let module = Rc::clone(&self.module);
        let fallback = Rc::clone(&self.fallback);
        let nesting = Rc::clone(&self.nesting);
        let f: torque_ts_vm::vm::NativeFn = Rc::new(move |vm, call| {
            let runner = Runner {
                module: Rc::clone(&module),
                fallback: Rc::clone(&fallback),
                nesting: Rc::clone(&nesting),
            };
            let args = call.argv.get(1..).unwrap_or(&[]).to_vec();
            let mut host = runner.host(vm, String::new());
            match runner.enter(|| module.call_defined(&mut host, id, &args)) {
                Ok(text) => Native::Str(text),
                Err(error) => Native::Fail(error),
            }
        });
        let package = (!package.is_empty()).then_some(package.as_bytes());
        self.vm.namespaces_mut().declare(
            namespace.as_bytes(),
            package,
            name.as_bytes(),
            Callable::Native {
                min: 0,
                max: 0,
                usage: Rc::from(usage),
                f,
            },
        );
        true
    }

    fn charge(&mut self, steps: u64) -> Result<(), VmError> {
        let paid = if self.vm.step_limit().is_none() {
            let used = self.fallback.get().saturating_add(steps);
            self.fallback.set(used);
            used <= TORQUE_DEFAULT_STEPS
        } else {
            self.vm.charge_work(steps.saturating_mul(WORK_PER_STEP))
        };
        if paid {
            Ok(())
        } else {
            Err(VmError::StepLimit)
        }
    }

    fn read_script(&mut self, path: &str) -> Result<Option<Vec<u8>>, VmError> {
        let lua = path
            .rsplit_once('.')
            .is_some_and(|(_, e)| e.eq_ignore_ascii_case("lua") || e.eq_ignore_ascii_case("luau"));
        if path.len() > TORQUE_MAX_TEXT || !is_plain_path(path) || !lua {
            return Ok(None);
        }
        Ok(read_checked(self.vm, path)?.filter(|source| source.len() <= TORQUE_MAX_SCRIPT))
    }

    fn print(&mut self, level: Level, text: &str) {
        // As the C ABI's `log`: a message past its cap is refused.
        if text.len() > TORQUE_MAX_TEXT {
            return;
        }
        match level {
            Level::Normal => self.vm.print(text.as_bytes()),
            Level::Warning => self.vm.warn(text.to_string()),
            Level::Error => self.vm.error(text.to_string()),
        }
    }
}

/// A console name the engine takes from a module: [`is_script_name`]'s, of at most [`TORQUE_MAX_NAME`] bytes.
fn is_name(name: &str) -> bool {
    name.len() <= TORQUE_MAX_NAME && is_script_name(name)
}

/// Whether `args` are arguments the engine takes from a module: at most [`TORQUE_MAX_ARGS`], each of at most
/// [`TORQUE_MAX_VALUE`] bytes.
fn are_args(args: &[Vec<u8>]) -> bool {
    args.len() <= TORQUE_MAX_ARGS && args.iter().all(|a| a.len() <= TORQUE_MAX_VALUE)
}
