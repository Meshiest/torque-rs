//! Small byte-level helpers shared by the torque-rs crates, so no crate writes its own: [`crc32`] and the running [`Crc32`],
//! [`hex`] and [`from_hex`], [`base64`], the work-budget callbacks ([`ChargeFn`], [`pay`], [`budget_charge`]), the cap error
//! [`CapExceeded`] and the id hasher [`idhash`].
//!
//! Std only and no dependency, so the crates that read untrusted files and datagrams can use it and stay free of external
//! crates. Every function is pure; the decoders ([`from_hex`], [`base64::decode`]) never trust their input and answer `None`
//! or an error for anything that is not exactly the text they define.
//!
//! # Maintaining
//!
//! One module per helper, each with its tests in `<module>_tests.rs`; `lib.rs` re-exports the flat names the callers use.
//! The crates that had their own copy keep their old paths as `pub use` of these items (`torque_net::crc`,
//! `torque_ts_text::crc32`, `torque_net::events::ChargeFn`, `torque_ghosts::{pay, free_charge}`,
//! `torque_server::net::web::http::base64`), so a change here is a change
//! to all of them.
//!
//! Rules a change must keep, each guarded by a test:
//! - `crc32` is the standard CRC-32 (check value `0xCBF43926` for `"123456789"`) and the eight-bytes-a-step update gives the same
//!   value as the definition, bit by bit, for every length and split (`the_wide_step_is_the_definition`). [`Crc32::raw`] is the
//!   state with no final complement: a wire checksum depends on it.
//! - `base64::encode` writes the RFC 4648 alphabet with padding. `base64::decode` accepts padding or none, checks the output size
//!   against `max` before it decodes anything, and rejects a length no encoder writes and any character outside the alphabet
//!   (`decoding_is_bounded_and_strict`): it reads untrusted `data:` URIs, so a change may tighten it, never loosen it.
//! - `from_hex` takes lower-case digits only, exactly `2 * N` of them (`hex_text_is_exact_lower_case`).
//! - [`pay`] refuses without changing the budget it was given; [`cell_charge`] takes nothing from a cell that cannot pay in
//!   full (`a_budget_refuses_what_it_cannot_pay_in_full`).
//!
//! Extending: a new helper goes in its own module with a test; keep the crate free of dependencies, and of anything a wasm
//! build cannot run.
//!
//! Credit: where Torque3D has the same rules, the CRC follows its MIT-licensed `core/crc.cpp` (Copyright (c) 2012 GarageGames,
//! LLC); the code here is this crate's own.

pub mod base64;
mod cap;
mod charge;
mod crc;
mod hex;
pub mod idhash;

pub use cap::CapExceeded;
pub use charge::{Charge, ChargeFn, OutOfBudget, budget_charge, cell_charge, free_charge, pay};
pub use crc::{Crc32, crc32, crc32_parts};
pub use hex::{from_hex, hex};
