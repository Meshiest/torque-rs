//! Attribute and derive macros: Rust functions as TorqueScript natives, and plugin ids (`#[derive(PluginId)]`, used
//! through `torque_engine`, which re-exports it).
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
