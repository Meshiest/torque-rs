//! The TGE game classes as console classes: the scene objects and their container ([`scene`]), the simulated classes' hosts
//! over torque-sim and torque-vehicle ([`sim`]), the shapes the datablocks load ([`shapes`]) and the datablocks' computed
//! values ([`dbvalues`]); a host reaches in through [`sim_classes`], [`scene::classes`], [`masks`], [`geometry`], [`ghost`] and
//! [`methods`]. Also: [`specs`] declares the scene, game, shape, datablock, effect, vehicle, mission and
//! map classes as TGE 1.4 does (each class's parent and its own static fields), so that a `Vm` without a host's class table
//! has them, and [`specs::install`] gives a `Vm` the ones it does not have yet. [`behaviour`] chains a Rust behaviour after the
//! one a class already has, as a C++ override calls its parent's.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/gameBase/gameBase.cpp`, `T3D/shapeBase.cpp`, `T3D/player.cpp` and `scene/sceneObject.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); `T3D/item.cpp`, `T3D/vehicles/vehicle.cpp`, `T3D/camera.cpp` and
//! `scene/sceneContainer.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.
//!
//! # Maintaining
//!
//! **Speed.** A server tick builds a collision phase for every Player, Item and vehicle. Rules a change must keep; the bench
//! that shows them is torque-trace-hash's `server` (64 AIPlayers standing among and walking beside 1,024 props, with the
//! allocations a tick it prints), and torque-vehicle's `tick` for the mover itself:
//! - A phase takes the plugins' shapes as one shared set ([`plugin_collision`], [`collide::source::CollisionSources::snapshot`]):
//!   a phase does not list the sources while their revisions hold.
//! - A shape's collision hull is made once per kept shape, detail and pose of its parts
//!   ([`scene::container_world::BuiltHulls`]): a phase that finds one pays the charges making it would, in the same order, so
//!   the budget runs out at the same charge either way. A shape's slots are made once ([`scene::container_world::hull_slots`]).
//! - The control chain ([`scene::camera::chain`]) is walked without an allocation (it runs per connection for every Player's
//!   tick), and the vehicle host keeps its buffers across sub-steps.
//! - Nothing here changes what is charged or returned: every line of `just trace-hash` stays.
//!
//! **One list of `onNewDataBlock` work.** `GameBase::onAdd` (torque-server's `game.rs`), `setDataBlock` and the refresh of a
//! redeclared datablock all run [`scene::object::new_datablock`], so a class that keeps state made from its datablock (the shape
//! instance, a Player's products, a vehicle's preload) adds its step there and must take a second datablock, and the same
//! datablock again, over the state the first made; the tests are in torque-server's `tests/set_datablock.rs` and
//! `tests/datablock_reload.rs`. `setDataBlock` also marks [`scene::object::DATABLOCK_MASK`] (GameBase's
//! bit 2, which the ghost codecs' `sim_mask` keeps as it is), and a client reads the new datablock from the next update of the
//! ghost. On a client it only writes the pointer: a ghost's object runs none of the server's classes.
//!
//! **Live datablocks** ([`scene::refresh`]). The flow: the static-write observer ([`scene::object::static_written`]) and the
//! `SimDataBlock` preload mark a datablock dirty (written or preloaded; one mark per datablock, the stronger kind wins); the
//! server's tick drains the set once before the process list's pass; the drain reads each dirty datablock's user count
//! ([`scene::ext::ObjectExt::db_users`], moved by `GameBase::onAdd`, `setDataBlock` and `onRemove`) and scans the process list
//! only when one has a user; the datablock class's chain refreshes the users (`EngineClass::datablock_changed`, root first, the
//! engine's on `GameBaseData`); `onReloaded()` follows. Invariants, each held by a test in `tests/datablock_reload.rs`:
//! - refreshed in place equals built fresh, field for field, for StaticShape, Item and Player (the differential tests over 200
//!   seeded edits): a class that caches a new field from its datablock adds it to the light refresh ([`scene::refresh`]'s
//!   `light`: a Player's words are [`sim::player::new_datablock`]'s `Some(old)` branch) or it goes stale;
//! - a write never rebuilds a shape and costs one light refresh per user per tick; a redeclare costs one heavy refresh per
//!   user whatever its count in the tick (`ten_thousand_writes...`, `a_thousand_redeclares...`);
//! - script threads keep sequence and position across a heavy refresh (`script_threads_keep...`);
//! - no scan while no dirty datablock has a user (`a_drain_scans_nothing...`), so a mission load stays cheap;
//! - a class with users and no refresh is warned about once (the unit tests in `scene/refresh.rs`).
//!
//! Extension points: a game chains its refresh with [`behaviour::chain`] on its datablock class; the drain's caps are
//! [`scene::refresh::MAX_REFRESHES_PER_DRAIN`] and [`scene::refresh::DRAIN_WORK`]. Check: `cargo test -p torque-game --lib refresh`
//! and `cargo test -p torque-server --test main datablock_reload`.

pub mod behaviour;
pub mod charge;
pub mod dbvalues;
pub mod geometry;
pub mod ghost;
pub mod library;
pub mod masks;
pub mod methods;
pub mod player_tether;
pub mod plugin_collision;
pub mod scene;
pub mod shapes;
pub mod sim;
pub mod sim_classes;
pub mod specs;
