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
