//! Attribute and derive macros: Rust functions as TorqueScript natives (`#[function]`, `#[methods]`), plugin ids
//! (`#[derive(PluginId)]`), preferences (`#[derive(Prefs)]`), console classes (`#[derive(EngineClass)]`) and plugin net
//! records (`#[derive(GhostRecord)]`, `#[derive(DatablockCodec)]`), used through `torque_engine`, which re-exports them.
//!
//! The entry points here only hand their tokens to `torque-macros-core`, where the code generation is tested.

use proc_macro::TokenStream;

/// Implements `torque_engine::PluginType` for a plugin type, with its stable id: the package's name and the type's as
/// a path (`my_game::ArenaPlugin`), or the one in `#[plugin(id = "...")]`. The engine crate's path is found in the
/// crate's manifest (`torque-engine`, or `torque` as `torque::engine`); `#[plugin(crate = path)]` gives it otherwise.
/// Used through `torque_engine::PluginId`, which re-exports it.
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType};
///
/// #[derive(PluginId)]
/// #[plugin(id = "torque.gamepad")]
/// struct Gamepad;
///
/// impl Plugin for Gamepad {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
///
/// assert_eq!(Gamepad::ID.as_str(), "torque.gamepad");
/// ```
#[proc_macro_derive(PluginId, attributes(plugin))]
pub fn derive_plugin_id(input: TokenStream) -> TokenStream {
    let package = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    let engine = torque_macros_core::find_engine();
    torque_macros_core::derive_plugin_id(input.into(), &package, &engine).into()
}

/// Declares a free function as a TorqueScript native: see `torque_engine::function`, which re-exports it with examples.
#[proc_macro_attribute]
pub fn function(args: TokenStream, item: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::function(args.into(), item.into(), &engine).into()
}

/// Declares the fns of an impl block as TorqueScript methods of a class: see `torque_engine::methods`, which
/// re-exports it with examples.
#[proc_macro_attribute]
pub fn methods(args: TokenStream, item: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::methods(args.into(), item.into(), &engine).into()
}

/// Declares a struct of typed preferences kept in `$Pref::...` globals: see `torque_engine::Prefs`, which re-exports it
/// with an example.
#[proc_macro_derive(Prefs, attributes(prefs, pref))]
pub fn derive_prefs(input: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::derive_prefs(input.into(), &engine).into()
}

/// Declares a console class as a struct with one typed field for each console field: see
/// `torque_engine::EngineClass`, which re-exports it with an example.
#[proc_macro_derive(EngineClass, attributes(class, field))]
pub fn derive_engine_class(input: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::derive_class(input.into(), &engine).into()
}

/// Declares a plugin's ghost record as a struct with one typed field for each value it sends: see
/// `torque_engine::GhostRecord`, which re-exports it with an example.
#[proc_macro_derive(GhostRecord, attributes(net))]
pub fn derive_ghost_record(input: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::derive_ghost_record(input.into(), &engine).into()
}

/// Declares a plugin's datablock record as a struct with one typed field for each value it sends: see
/// `torque_engine::DatablockCodec`, which re-exports it with an example.
#[proc_macro_derive(DatablockCodec, attributes(net))]
pub fn derive_datablock_codec(input: TokenStream) -> TokenStream {
    let engine = torque_macros_core::find_engine();
    torque_macros_core::derive_datablock_codec(input.into(), &engine).into()
}

/// Leaves the item it is written on exactly as it is.
///
/// ```
/// #[torque_macros::passthrough]
/// fn set_dead_zone(zone: f32) -> f32 { zone }
/// ```
#[proc_macro_attribute]
pub fn passthrough(args: TokenStream, item: TokenStream) -> TokenStream {
    torque_macros_core::passthrough(args.into(), item.into()).into()
}
