//! Lua in a browser build: the interface a separately loaded Luau module plugs into, and the stand-in used until there is
//! one.
//!
//! Native extensions do not load in a browser, so a page's Lua is a second WebAssembly module (Luau with the same sandbox
//! as the native extension's), fetched by the page like any of its files and bridged through the page's JavaScript.
//! [`WebLuaPlugin`] installs the `.lua` and `.luau` languages in every script engine and hands their files to a
//! [`LuaModule`]; the module calls the script engine back through the [`LuaHost`] it is handed, which offers what the C
//! ABI's script functions offer a native extension, with the same checks (`exec`'s for every file it reads, the packages
//! its definitions may go into, the step budget). Until a page has the module, [`MissingLuaModule`] stands in: a Lua file
//! prints that it cannot run, and nothing else happens.
//!
//! The plugin builds on every target, so its host is tested natively with a module written in Rust.

use std::cell::Cell;
use std::rc::Rc;

use torque_engine::{ClientStage, EngineBuilder, Plugin, ServerStage, Side};
use torque_plugin_abi::TORQUE_DEFAULT_STEPS;
use torque_ts_vm::ns::Callable;
use torque_ts_vm::policy::ScriptLanguage;
use torque_ts_vm::vm::{Level, MAX_CALL_DEPTH, Native, Vm, VmError, WORK_PER_STEP};

use crate::language::{VmContext, is_plain_path, read_checked};

/// The call-depth units a call back from the module costs: its frames are the page's (the module's and the bridge's),
/// out of the engine's sight, so a fixed share of the budget stands in for them.
const CALL_BACK_COST: usize = 8;

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

/// What the engine offers a [`LuaModule`] during one of its calls: the C ABI's script functions, as Rust.
pub trait LuaHost {
    /// The realm's number (the script engine's, never another's) and side.
    fn realm(&self) -> (u64, Side);
    /// During a `run_file`: the package the file's definitions go into (a mod's), or the empty text.
    fn package(&self) -> &str;
    /// Runs the console function `namespace::name`.
    fn call_function(
        &mut self,
        namespace: &str,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError>;
    /// Runs the method `name` of `object`.
    fn call_method(
        &mut self,
        object: u32,
        name: &str,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError>;
    /// Whether `object` exists and has a method `name`.
    fn has_method(&self, object: u32, name: &str) -> bool;
    /// The object `name` names.
    fn find_object(&self, name: &[u8]) -> Option<u32>;
    /// The field `name[index]` of `object`; `None` when the object does not exist.
    fn field(&self, object: u32, name: &[u8], index: &[u8]) -> Option<Vec<u8>>;
    /// Sets the field `name[index]` of `object`: false when the object does not exist.
    fn set_field(&mut self, object: u32, name: &[u8], index: &[u8], value: &[u8]) -> bool;
    /// A global's value (`name` with its `$`).
    fn global(&self, name: &str) -> Vec<u8>;
    /// Sets a global.
    fn set_global(&mut self, name: &str, value: &[u8]);
    /// Defines a console function whose calls come to [`LuaModule::call_defined`] with `id`: false when the names or the
    /// package are refused.
    fn define_function(&mut self, namespace: &str, name: &str, package: &str, id: u64) -> bool;
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

/// Lua through a [`LuaModule`] (by default [`MissingLuaModule`]): the browser build's counterpart of the native Lua
/// extension. Its id is `torque.lua`, the extension's, so a mod's `requires_plugins = ["torque.lua"]` holds for both.
#[derive(torque_engine::PluginId)]
#[plugin(id = "torque.lua")]
pub struct WebLuaPlugin {
    module: Rc<dyn LuaModule>,
}

impl Default for WebLuaPlugin {
    fn default() -> WebLuaPlugin {
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

    /// Runs `f` as one call into the module: the outermost starts the fallback budget again.
    fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        if self.nesting.get() == 0 {
            self.fallback.set(0);
        }
        self.nesting.set(self.nesting.get() + 1);
        let out = f();
        self.nesting.set(self.nesting.get() - 1);
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
    /// Runs script for the module: charged [`CALL_BACK_COST`] call-depth units, and refused past the limit.
    fn back(
        &mut self,
        run: impl FnOnce(&mut Vm) -> Result<Vec<u8>, VmError>,
    ) -> Result<Vec<u8>, VmError> {
        if self.vm.call_depth().saturating_add(CALL_BACK_COST) > MAX_CALL_DEPTH {
            return Err(VmError::CallDepth);
        }
        self.vm.with_extra_depth(CALL_BACK_COST, run)
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
    ) -> Result<Vec<u8>, VmError> {
        if let Err(reason) = crate::call_policy::check(self.vm, namespace, name, false, args) {
            self.vm.error(reason);
            return Ok(Vec::new());
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
    ) -> Result<Vec<u8>, VmError> {
        if let Err(reason) = crate::call_policy::check(self.vm, "", name, true, args) {
            self.vm.error(reason);
            return Ok(Vec::new());
        }
        self.back(|vm| {
            let result = vm.call_method(object, name.as_bytes(), args)?;
            crate::call_policy::note_deferred(vm, name, &result);
            Ok(result)
        })
    }

    fn has_method(&self, object: u32, name: &str) -> bool {
        self.vm.objects().get(object).is_some()
            && self.vm.method_origin(object, name.as_bytes()).is_some()
    }

    fn find_object(&self, name: &[u8]) -> Option<u32> {
        self.vm.objects().find(name)
    }

    fn field(&self, object: u32, name: &[u8], index: &[u8]) -> Option<Vec<u8>> {
        self.vm.objects().get(object)?;
        Some(self.vm.script_field(object, name, index))
    }

    fn set_field(&mut self, object: u32, name: &[u8], index: &[u8], value: &[u8]) -> bool {
        if name.is_empty() || self.vm.objects().get(object).is_none() {
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
        self.vm.global(name).unwrap_or_default()
    }

    fn set_global(&mut self, name: &str, value: &[u8]) {
        self.vm.set_global_text(name.as_bytes(), value);
    }

    fn define_function(&mut self, namespace: &str, name: &str, package: &str, id: u64) -> bool {
        let is_name = |n: &str| {
            n.bytes().next().is_some_and(|b| !b.is_ascii_digit())
                && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        };
        if !is_name(name) || !(namespace.is_empty() || is_name(namespace)) {
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
                usage: Rc::from(""),
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
        if !is_plain_path(path) || !lua {
            return Ok(None);
        }
        read_checked(self.vm, path)
    }

    fn print(&mut self, level: Level, text: &str) {
        self.vm.log_bytes(level, text.as_bytes());
    }
}
