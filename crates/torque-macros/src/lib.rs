//! Attribute and derive macros that turn Rust functions into TorqueScript natives.
//!
//! The entry points here only hand their tokens to `torque-macros-core`, where the code generation is tested.

use proc_macro::TokenStream;

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
