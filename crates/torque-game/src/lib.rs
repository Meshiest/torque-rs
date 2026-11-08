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

pub mod behaviour;
pub mod charge;
pub mod dbvalues;
pub mod geometry;
pub mod ghost;
pub mod library;
pub mod masks;
pub mod methods;
pub mod plugin_collision;
pub mod scene;
pub mod shapes;
pub mod sim;
pub mod sim_classes;
pub mod specs;
