//! The integration tests, linked as one test program: one module per file.

#[macro_use]
mod common;

mod client_script;
mod gui_layout;
mod library;
// Also a module of the realm-gate example and the lua-check crate, so it keeps its own `mod common;`.
#[path = "lua.rs"]
mod lua;
