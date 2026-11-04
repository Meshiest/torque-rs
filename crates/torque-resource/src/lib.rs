//! The resource layer of a Torque host: a game folder read-only with its zips mounted as folders and the files the scripts
//! write kept in memory ([`files`]: lookups, the search list, the written files, case rules), over the game folder indexed once
//! with its zips mounted ([`folder`]) and a hardened zip reader ([`zips`]). What mounts where and what the resource manager refuses is the host's [`files::MountPolicy`].
//!
//! The behaviour follows TGE's resource manager and zip aggregate (`core/resManager.cc` and `core/zipAggregate.cc`, each
//! `Copyright (C) GarageGames.com, Inc.`), released under the MIT licence as Torque3D's predecessor; the code here is this
//! crate's own.

pub mod files;
pub mod folder;
pub mod zips;
