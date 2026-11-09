//! The resource layer of a Torque host: a game folder read-only with its zips mounted as folders and the files the scripts
//! write kept in memory ([`files`]: lookups, the search list, the written files, case rules), over the game folder indexed once
//! with its zips mounted ([`folder`]) and a hardened zip reader ([`zips`]). What mounts where and what the resource manager refuses is the host's [`files::MountPolicy`].
//!
//! The behaviour follows TGE's resource manager and zip aggregate (`core/resManager.cc` and `core/zipAggregate.cc`, each
//! `Copyright (C) GarageGames.com, Inc.`), released under the MIT licence as Torque3D's predecessor; the code here is this
//! crate's own.
//!
//! # Maintaining
//!
//! **One reader for plain files** ([`plain`]): the format crates, the FGD reader, the glTF side files and the folder host read a
//! file a user, a mod or a download may have put there through `plain::open_plain` / `read_plain` / `read_within`: opened
//! without following a link (`O_NOFOLLOW` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` on Windows), the handle checked (a regular file,
//! no reparse point), the length taken from the handle, the read stopped one byte past the cap and past a listed length. A new
//! reader of such files uses it rather than `std::fs::read`. Tests: `tests/plain.rs` (`a_file_reads_whole_within_its_cap_and_its_listed_length`,
//! `a_folder_and_a_link_are_not_plain_files`, which makes a symbolic link where the machine can).

pub mod files;
pub mod folder;
pub mod plain;
pub mod zips;
