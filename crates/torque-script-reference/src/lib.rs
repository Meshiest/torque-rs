//! Writes the guide's script reference from the script engines an engine makes.
//!
//! [`engines::reference`] builds an engine of `DefaultPlugins` (and one for each opt-in plugin the reference covers), makes a
//! server and a client from each, and reads their natives, console classes and prefs ([`collect`]); [`render::pages`] turns
//! that into the pages. An engine registers the RPC natives only when a plugin registers an RPC, so one more engine
//! with a sample plugin that does supplies those natives and the RPC page's shapes of the fields. Nothing in the output
//! depends on the machine or the run, so the pages can be checked in and compared.

pub mod collect;
pub mod engines;
pub mod model;
pub mod render;

#[cfg(test)]
mod collect_tests;
#[cfg(test)]
mod engines_tests;
#[cfg(test)]
mod render_tests;
