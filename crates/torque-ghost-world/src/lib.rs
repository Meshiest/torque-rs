//! The client's world of ghosts: the record world ([`world::ClientWorld`]) over a game's classes and records
//! ([`set::ClientSet`]) and what the records drive, the mounted images' state machine ([`images`]), the script threads
//! ([`threads`]) and the effects queue ([`effects`]). A game's view of the world wraps the record world and adds its own parts.
//!
//! [`tge`] is TGE 1.4's own set ([`tge::TgeClientSet`]) over `torque-ghosts`' TGE records, with a client ghost table for it.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - The world follows at most [`world::MAX_WORLD_GHOSTS`] ghosts whatever ghost table a server's accept allows, the ones past it
//!   counted (`a_24_bit_ghost_table_is_followed_up_to_the_worlds_cap`).

pub mod effects;
pub mod images;
pub mod set;
pub mod tge;
pub mod threads;
pub mod warp;
pub mod world;

pub use set::{
    ClientDatablock, ClientRecord, ClientSet, ClientTail, GhostHooks, GhostPart, SentGhost,
    SentTable,
};
pub use world::{ClientWorld, Ghost};
