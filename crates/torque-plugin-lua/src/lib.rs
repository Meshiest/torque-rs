//! Lua scripting for torque-rs mods: [Luau](https://luau.org) (Lua 5.1 with types, `+=`, `continue` and string
//! interpolation), as a native extension the engine loads from its `plugins/` folder. A mod's `.lua` (or `.luau`) files
//! run at the mod's stages, as its TorqueScript does, and a game's own `.lua` files run with `exec`.
//!
//! **Realms.** Each script engine, a server's or a client's, has its own Lua state: a script runs in one realm and sees
//! only that realm's console functions, objects and globals (`ts.realm` is `"server"` or `"client"`, and `SERVER` or
//! `CLIENT` is true). Each mod has its own global table in its realm, read through to the shared, read-only built-ins.
//!
//! **What a script can do** (the `ts` table): call console functions and methods (`ts.call("echo", "hi")`,
//! `obj:setTransform(...)`), find objects (`ts.object("LocalClientConnection")`) and read and write their fields
//! (`obj.health`, `obj.health = 50`), read and set globals (`ts.global("$Pref::Server::Name")`), define console functions
//! and methods that TorqueScript calls back (`ts.define("Player::onLand", function(this) ... end)`), and print
//! (`print`, `ts.warn`). Values cross as text, as everything in TorqueScript does.
//!
//! **The sandbox.** No `io`, no `os` but `os.clock`, no `loadstring` and no bytecode, no `debug`, no `getfenv` or
//! `setfenv`, no `package`; `require` loads only `.lua` files of the requiring mod's own folder. The built-ins and the
//! shared tables are read-only. A script's work is charged to the script engine's step limit (Luau interrupts at every
//! call and loop), so a runaway loop stops with an error as a runaway TorqueScript loop does, and each realm's memory is
//! capped (`$Pref::Lua::MemoryLimit`, in MiB). The builtins that work through a whole string, table or buffer at once
//! (`string.rep`, `table.concat`, `buffer.fill`, ...) are charged by the bytes they handle. The engine refuses the console
//! functions and methods that run code from text or touch files (`exec`, `eval`, `call`, `save`, ..., under any namespace;
//! its call policy, `torque_plugin_loader::call_policy`). Lua files come only from local mods and the game: a file a server
//! sent never runs, exactly as for TorqueScript.

#![warn(missing_docs)]

mod api;
#[cfg(feature = "client-script")]
pub mod client_script;
mod realm;
mod require;
mod value;
// The client-script guest's module has the realm's entry points instead (`client_script`).
#[cfg(all(target_os = "wasi", not(feature = "client-script")))]
mod web;

use std::sync::Arc;

use torque_plugin_sdk::{Call, Extension, Limit, Registrar, ScriptLanguage, Side};

pub use realm::MEMORY_LIMIT_PREF;

/// The Lua extension: the script languages of `.lua` and `.luau` files.
#[derive(Default)]
pub struct LuaExtension;

impl Extension for LuaExtension {
    const ID: &'static str = "torque.lua";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    fn build(&self, ext: &mut Registrar) {
        ext.script().global(MEMORY_LIMIT_PREF, "64");
        let realms = Arc::new(realm::Realms::default());
        for extension in ["lua", "luau"] {
            ext.script_language(
                Side::Both,
                extension,
                Language {
                    realms: Arc::clone(&realms),
                },
            );
        }
    }
}

/// One file extension's language: every one shares the realms.
struct Language {
    realms: Arc<realm::Realms>,
}

impl ScriptLanguage for Language {
    fn run_file(&self, call: Call<'_>, path: &str, source: &[u8]) -> Result<(), Limit> {
        self.realms.run_file(call, path, source)
    }

    fn call_defined(&self, call: Call<'_>, id: u64) -> Result<Vec<u8>, Limit> {
        self.realms.call_defined(call, id)
    }

    fn vm_dropped(&self, vm: u64) {
        self.realms.drop_realm(vm);
    }
}

// The library a game loads; a browser build's module has the page's entry points instead (`web`).
#[cfg(not(target_os = "wasi"))]
torque_plugin_sdk::export!(LuaExtension);

torque_plugin_sdk::init_fn!(linked_init, LuaExtension);

/// The extension's entry point, for a game (or a test) that links the extension in instead of loading its library:
/// `ExtensionPlugin::from_init(torque_plugin_sdk::abi::TORQUE_ABI_VERSION, torque_lua::INIT)`.
pub const INIT: torque_plugin_sdk::abi::TorquePluginInitFn = linked_init;
