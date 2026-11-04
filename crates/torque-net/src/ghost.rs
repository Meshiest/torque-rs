//! The NetObject model and the ghost manager.
//!
//! A pure state machine per connection over two traits the host implements ([`source::ObjectSource`], [`source::GhostScripts`]):
//! no socket, no script, no clock but the `now` it is handed.
//!
//! - `table`: the three-region ghost array (`mGhostArray`) kept sparse. **Ids are Torque's**: a ghost's wire id is its GhostInfo struct
//!   number; a new ghost takes the front of the free region, a freed ghost goes back to the front (LIFO reuse), ghost-always
//!   objects take `Max - n + j`, and every swap follows the array's, so the send order follows.
//! - [`manager`]: scope (the full query, by default with every packet and dropping the ghosts it no longer finds, `objectInScope`
//!   with the host's dependents), the priority pass (`dQsort` through [`source::ObjectSource::sort_pending`], a stable sort unless the host gives another), the
//!   ghost section of a data packet, `GhostRef` notify chains (ack and drop, re-armed masks), `activateGhosting` / `resetGhosting`,
//!   the ghost `ConnectionMessage`s and the natives. A policy may space the full queries out (with a walk of its own list between
//!   them), admit and drop ghosts and send the pending ones in another order.
//! - [`client`]: a reference ghost client over a [`client::GhostCodec`] (the host's object codec plugs in there).
//! - [`policy`]: the [`GhostPolicy`] a game sets its ghosting rules through (when the full query runs, admission, drops, the
//!   order, scope rules, an implied class form);
//!   [`TorqueGhosts`], the default, is plain Torque.
//!
//! **Additions to the standard behaviour**: a second `activate` without a reset is refused; a ghost-always event made before a reset
//! packs its empty form; a record that overflows the 12,000-bit stream is rolled back and skipped; a ghost-always set above the limit
//! is refused; the dependents recursion, the refs in flight and the work per packet are capped; a scan that runs out of budget resumes
//! with the next packet; `pack_update` must be repeatable.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `sim/netObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the
//! code here is this crate's own.

pub mod client;
pub mod manager;
pub mod policy;
pub mod source;
mod table;

pub use client::{
    ClassForm, ClientEvent, GhostChange, GhostClient, GhostCodec, GhostEventReader, SectionReport,
};
pub use manager::{
    GhostConfig, GhostError, GhostEvent, GhostManager, GhostPackCtx, GhostView, KILL_PRIORITY,
    MessageOutcome, SectionBudget, SectionStats,
};
pub use policy::{GhostPolicy, ObjectPolicy, PendingOrder, ScopeRadius, TableCounts, TorqueGhosts};

pub use source::{
    CamInfo, GhostScripts, NoScripts, ObjId, ObjectSource, SCOPE_OVERRIDE, flags, net_flags,
    priority_compare, scope_radius_used,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world;
