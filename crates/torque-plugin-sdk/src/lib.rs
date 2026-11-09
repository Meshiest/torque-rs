//! Write a native extension of the engine in Rust: a library the engine loads at run time from its `plugins/` folder,
//! over the engine's C ABI ([`torque_plugin_abi`]), that adds console functions, methods, globals and render passes the
//! way a compiled plugin does.
//!
//! **A native extension runs with the game's full rights**: it can read and write the player's files, use the network
//! and do anything else the game can. The engine loads one only from a local folder the player put it in, only once the
//! player has approved that exact file (its SHA-256 hash), and never one a server sends.
//!
//! An extension is a `cdylib` crate with a type that implements [`Extension`] and the [`export!`] line:
//!
//! ```
//! use torque_plugin_sdk::{Extension, Registrar, Space, Stage, function, natives};
//!
//! /// greets someone
//! #[function]
//! fn sample_greet(name: &str) -> String {
//!     format!("Hello, {name}!")
//! }
//!
//! #[derive(Default)]
//! struct Sample;
//!
//! impl Extension for Sample {
//!     const ID: &'static str = "example.native-sample";
//!     const VERSION: &'static str = "0.1.0";
//!
//!     fn build(&self, ext: &mut Registrar) {
//!         ext.script().natives(natives![sample_greet]);
//!         ext.script().function("Sample", "twice", &["n"], "doubles a number", |n: i32| n * 2);
//!         ext.render().pass(Stage::Overlay, || {
//!             |_frame: &torque_plugin_sdk::PassFrame, out: &mut torque_plugin_sdk::Triangles| {
//!                 let corners = [[-0.9, -0.9, 0.0], [-0.7, -0.9, 0.0], [-0.7, -0.7, 0.0], [-0.9, -0.7, 0.0]];
//!                 out.quad(Space::Clip, corners, [1.0, 0.5, 0.0, 1.0]);
//!             }
//!         });
//!     }
//! }
//!
//! // In the extension crate: `torque_plugin_sdk::export!(Sample);`
//! # torque_plugin_sdk::init_fn!(sample_init, Sample);
//! ```
//!
//! The [`function`] attribute is the engine's own `#[function]`: in a crate that depends on this SDK (and not on the
//! engine) it declares the native through the SDK, with the same names, argument counts and usage text a compiled
//! plugin's would have. Parameters are script arguments (`i32`, `u32`, `f32`, `bool`, `&str`, `String`, `&[u8]`,
//! `Option<T>`, [`Rest`]) read by the engine's own rules, and the injected [`Call`] (the console, globals) and [`This`]
//! (a method's object). A function returns `()`, a number, a truth value, text, or a `Result` whose error is printed.
//!
//! **Threads.** The engine calls an extension from any of its threads, one call at a time, and refuses a call that would
//! re-enter it on the thread already inside it: natives are `Send + Sync` and passes `Send`, and thread-local storage
//! does not carry from one call to the next.
//!
//! **Script languages.** An extension can run files of another language: [`Registrar::script_language`] declares a
//! [`ScriptLanguage`] for a file extension, and the engine's `exec` hands it those files (a mod's, the game's; never one a
//! server sent). During its calls it runs the script engine's functions and methods ([`Call::call_function`],
//! [`Call::call_method`]), reads and writes objects' fields, defines console functions that call it back
//! ([`Call::define_function`]) and charges its work to the step budget ([`Call::charge`]).
//!
//! **Panics** are caught at every entry from the engine: the panic's message goes to the engine, which disables the
//! extension (its functions then print an error and return the empty string; its passes stop drawing). Build the
//! extension with the default `panic = "unwind"`: with `panic = "abort"` a panic ends the game.

#![warn(missing_docs)]

// The `#[function]` attribute names this crate by its path, which inside the crate (its tests) is this.
extern crate self as torque_plugin_sdk;

mod ffi;
pub mod language;
pub mod param;
pub mod render;
pub mod script;

pub use language::{CallError, Definition, Limit, ScriptLanguage, VmInfo};
pub use param::{ArgKind, IntoReturn, Param, ParamKind, Rest, This};
pub use render::{Pass, PassFrame, RenderRegistry, Space, Stage, Triangles, Vertex};
pub use script::{Effects, IntoNative, NativeDecl, Natives, ScriptRegistry, TypedNative};
/// Declares a free function as a console function of the extension, for [`ScriptRegistry::natives`] (listed with
/// [`natives!`]): the engine's `#[function]` attribute, with the same options (`ns`, `name`, `usage`, `description`,
/// `dev`), found through this crate. `crate = torque_plugin_sdk` names this crate in a crate that also depends on the
/// engine.
pub use torque_macros::function;
pub use torque_plugin_abi as abi;

use torque_plugin_abi::{
    TORQUE_LOG_ERROR, TORQUE_LOG_INFO, TORQUE_LOG_WARN, TORQUE_SIDE_BOTH, TORQUE_SIDE_CLIENT,
    TORQUE_SIDE_SERVER, TorqueApi, TorqueCall,
};

/// A native extension: what [`export!`] makes the library's entry point build.
///
/// [`build`](Extension::build) runs once, when the engine loads the library, on the type's `Default` value; it records
/// what the extension adds into the [`Registrar`], as a compiled plugin's `build` records into the engine's builder.
pub trait Extension: Default + 'static {
    /// The extension's stable id, such as `example.native-sample`: ASCII letters, digits, `_`, `-`, `.` and `:`. It is
    /// its plugin id in the engine, so config files and servers name it by this.
    const ID: &'static str;
    /// Its version, such as `0.1.0`: shown when the player approves it.
    const VERSION: &'static str;
    /// `Some(version)` when it changes what goes over the network: a server and a client must then both have it, at
    /// the same version.
    const NET_VERSION: Option<u32> = None;

    /// Records what the extension adds.
    fn build(&self, ext: &mut Registrar);
}

/// Which script engines a declaration goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Every server's and every client's.
    Both,
    /// Every server's.
    Server,
    /// Every client's.
    Client,
}

impl Side {
    pub(crate) const fn code(self) -> u32 {
        match self {
            Side::Both => TORQUE_SIDE_BOTH,
            Side::Server => TORQUE_SIDE_SERVER,
            Side::Client => TORQUE_SIDE_CLIENT,
        }
    }
}

/// What an extension's [`build`](Extension::build) records into: its console functions, methods, globals and passes,
/// handed to the engine as they are declared.
pub struct Registrar<'a> {
    pub(crate) api: &'static TorqueApi,
    pub(crate) refused: Vec<String>,
    pub(crate) _life: std::marker::PhantomData<&'a ()>,
}

impl<'a> Registrar<'a> {
    /// Declares console functions, methods and globals for every script engine, servers' and clients'.
    pub fn script(&mut self) -> ScriptRegistry<'_, 'a> {
        ScriptRegistry {
            registrar: self,
            side: Side::Both,
            effects: Effects::NONE,
        }
    }

    /// Declares them for every server's script engine.
    pub fn server_script(&mut self) -> ScriptRegistry<'_, 'a> {
        ScriptRegistry {
            registrar: self,
            side: Side::Server,
            effects: Effects::NONE,
        }
    }

    /// Declares them for every client's script engine.
    pub fn client_script(&mut self) -> ScriptRegistry<'_, 'a> {
        ScriptRegistry {
            registrar: self,
            side: Side::Client,
            effects: Effects::NONE,
        }
    }

    /// Declares render passes.
    pub fn render(&mut self) -> RenderRegistry<'_, 'a> {
        RenderRegistry { registrar: self }
    }

    /// Declares `language` for the files whose extension is `extension` (without the dot: `lua`), in the script engines
    /// of `side`: the engine's `exec` hands them to it.
    pub fn script_language(&mut self, side: Side, extension: &str, language: impl ScriptLanguage) {
        self.language(side, extension, Box::new(language));
    }

    /// Writes `message` to the engine's log.
    pub fn log(&mut self, message: &str) {
        ffi::log(self.api, self.api.init, TORQUE_LOG_INFO, message);
    }

    /// Records a declaration the engine (or the SDK) refused: the engine is told when the build ends, and does not
    /// load the extension.
    pub(crate) fn refuse(&mut self, message: String) {
        self.refused.push(message);
    }
}

/// One call of a console function or method: the console, globals and the call's object. A native takes it as a
/// parameter; it is valid for the call only.
#[derive(Clone, Copy)]
pub struct Call<'a> {
    pub(crate) api: &'static TorqueApi,
    pub(crate) handle: TorqueCall,
    pub(crate) _life: std::marker::PhantomData<&'a ()>,
}

impl<'a> Call<'a> {
    /// The call's handle: what the engine's table names it by.
    pub fn handle(&self) -> TorqueCall {
        self.handle
    }

    /// The engine's function table.
    pub fn api(&self) -> &'static TorqueApi {
        self.api
    }

    /// Prints `message` on the console of the script engine that made the call.
    pub fn print(&self, message: &str) {
        ffi::log(self.api, self.handle, TORQUE_LOG_INFO, message);
    }

    /// Prints a warning on that console.
    pub fn warn(&self, message: &str) {
        ffi::log(self.api, self.handle, TORQUE_LOG_WARN, message);
    }

    /// Prints an error on that console.
    pub fn error(&self, message: &str) {
        ffi::log(self.api, self.handle, TORQUE_LOG_ERROR, message);
    }

    /// The global variable `name` (with its `$`), empty when it is not set.
    pub fn global(&self, name: &str) -> Vec<u8> {
        ffi::get_global(self.api, self.handle, name).unwrap_or_default()
    }

    /// Sets the global variable `name` (with its `$`): false when the engine refused it.
    pub fn set_global(&self, name: &str, value: &[u8]) -> bool {
        ffi::set_global(self.api, self.handle, name, value)
    }
}

/// Makes the library's two exports for the extension type `$ext`: `torque_plugin_abi_version` and
/// `torque_plugin_init`. Write it once, in the extension crate's `lib.rs`.
#[macro_export]
macro_rules! export {
    ($ext:ty) => {
        /// The ABI version this extension was built against: read by the engine before it runs any of its code.
        #[allow(non_upper_case_globals)]
        #[unsafe(no_mangle)]
        pub static torque_plugin_abi_version: u32 = $crate::abi::TORQUE_ABI_VERSION;

        /// The extension's entry point.
        ///
        /// # Safety
        ///
        /// The engine calls it once, with its function table and a place for the extension's info.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn torque_plugin_init(
            api: *const $crate::abi::TorqueApi,
            out: *mut $crate::abi::TorquePluginInfo,
        ) -> $crate::abi::TorqueStatus {
            // SAFETY: the engine hands a valid table and info, as the ABI says; `init` checks both for null and the
            // table's version before it reads anything else.
            unsafe { $crate::__private::init::<$ext>(api, out) }
        }
    };
}

/// Makes an entry point named `$name` for the extension type `$ext` without exporting it: for an extension linked into
/// the game itself (the engine's loader takes the function), or for tests that hold several extensions in one binary.
#[macro_export]
macro_rules! init_fn {
    ($name:ident, $ext:ty) => {
        /// The entry point of an extension linked in.
        ///
        /// # Safety
        ///
        /// Called with the engine's function table and a place for the extension's info.
        unsafe extern "C" fn $name(
            api: *const $crate::abi::TorqueApi,
            out: *mut $crate::abi::TorquePluginInfo,
        ) -> $crate::abi::TorqueStatus {
            // SAFETY: as for `export!`.
            unsafe { $crate::__private::init::<$ext>(api, out) }
        }
    };
}

#[doc(hidden)]
pub mod __private {
    pub use crate::ffi::init;
}

#[cfg(test)]
mod tests;
