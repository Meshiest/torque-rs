//! Test support shared by the torque-rs crates, so no test writes its own: a seeded [`Rng`], the fuzz loop and the rounds a
//! fuzz test runs ([`fuzz`]), byte mutators ([`mutate`]), a [`FakeClock`], a [`TempFolder`] that cleans up after itself and,
//! behind the `zip` feature, `zip_of`.
//!
//! Dev only. A crate lists `torque-testkit` under `[dev-dependencies]` and nowhere else, so none of it ships. The crate names
//! no other workspace crate, which is what lets every crate use it, the leaves included. What needs a type of another crate
//! (a map view over ghosts, a box world over collision, a rig over an app) lives with that type instead.
//!
//! # Maintaining
//!
//! One module per helper, each with its own tests; `lib.rs` re-exports the flat names the callers use.
//!
//! Rules a change must keep, each guarded by a test:
//! - [`Rng::new`] is a xorshift64 (13/7/17) and [`Rng::star`] a xorshift64* (12/25/27) with the seed scrambling written in their
//!   docs: seeded tests and pinned goldens depend on the exact bits (`the_generators_keep_their_bits`). A new generator gets a
//!   new constructor; an existing one never changes.
//! - [`SplitMix64`] matches the reference vector (`splitmix_matches_the_reference_vector`).
//! - [`fuzz::run`] puts the failing case's number on stderr and lets the panic go on; it never swallows one.
//! - Every manifest in the workspace lists this crate under `[dev-dependencies]` only (`no_crate_ships_the_testkit`).
//!
//! Extending: a helper that three tests have each written goes here with a test of its own; keep the crate free of workspace
//! crates (a test that needs one belongs in that crate's `tests/common`) and of dependencies beyond a feature-gated one.
//!
//! Check: `cargo test -p torque-testkit --all-features`.

mod clock;
mod folder;
pub mod fuzz;
pub mod mutate;
mod rng;
#[cfg(feature = "zip")]
mod zip;

pub use clock::FakeClock;
pub use folder::TempFolder;
pub use rng::{Rng, SplitMix64};
#[cfg(feature = "zip")]
pub use zip::{zip_of, zip_stored_of};

#[cfg(test)]
mod tests;
