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
//! - A server's records are read within the datagram's work budget (`tge::TgeCodec::set_budget`,
//!   `torque_ghosts::datagram_budget`; `strings_at_their_greatest_expansion_stop_at_the_datagrams_budget` in torque-ghosts),
//!   and an image's state chain is charged to the clock step's budget
//!   (`images::tests::looping_state_chains_are_charged_to_the_tick_budget`).
//! - Hostile orders of records (replayed, dropped, reordered, cut, spliced) never panic the table or the world
//!   (`hostile_ghost_sequences_never_panic_the_client_or_its_world` in `tests/session.rs`, `TORQUE_FUZZ_GHOSTS`).
//! - A server's scene-hierarchy links ([`links`]) go through the same checks as a server's tree, every pose through the
//!   quantizer's range check, and at most [`links::MAX_WAITING`] wait, each at most [`links::WAIT_MS`]; a link never names a
//!   ghost the world does not hold, and a parent's ghost that goes leaves its children where they were drawn (`links::tests`;
//!   torque-plugin-hierarchy's `tests/net_fuzz.rs`, `TORQUE_FUZZ_HIERARCHY_NET`). A waiting link costs nothing until its parent
//!   arrives or its time runs out (found by parent and by age, never by a walk of what waits).
//! - [`ClientWorld::world_pose`] and the renderer (`torque_render_scene::mounts`) draw a child the way the server composes it,
//!   by `torque_hierarchy::compose` (torque-plugin-hierarchy's `tests/net.rs`: within a millimetre over 600 ticks with a fifth
//!   of the datagrams lost). A plugin's lookup of where a ghost is (`torque_engine::net_classes::ClientGhostLookup`, from the
//!   client's connection) is [`ClientWorld::world_pose`] too (torque-plugin-rope's `tests/hierarchy_anchor.rs`). The control
//!   object the client predicts is where the prediction draws it (`control_render`), a controlled child on its parent's drawn
//!   pose (torque-plugin-hierarchy's `tests/turret.rs`).
//!   [`ClientWorld::tick_pose_in`] gives a prediction tick's pose through the same compose, each local
//!   pose as last sent, a ghost the caller runs where it runs it and any other ghost nothing carries at its record's pose;
//!   each ghost a tick's chains reach is worked out once ([`world::TickPoses`]) and charged, and a refused charge stops the
//!   walk (torque-plugin-hierarchy's `tests/ride.rs`: within a millimetre of the server's pose at the latest record's tick;
//!   torque-client's `predict::linked_tests`: the largest tree the links hold within one tick's budget).

pub mod effects;
pub mod images;
pub mod links;
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
