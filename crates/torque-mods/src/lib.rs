//! Local mods for torque-rs: folders and zips of scripts and assets, mounted over a game's files and run as TorqueScript
//! packages, within hard limits.
//!
//! [`ModsPlugin`] (`torque.mods`) does this when a server's or a client's script engine is made:
//!
//! 1. The roots are searched (by default the game's `mods` folder): every folder and every `.zip` is a candidate, read
//!    into a [`ModPackage`] under the [`ModLimits`] and the path rules.
//! 2. A [`ModLayout`] recognizes each candidate as a mod and reads it into a [`ModInfo`] (by default from `mod.toml`).
//! 3. Mods whose prerequisites are not met are left out ([`ModPrereqs`]), the rest are put in load order ([`ModOrder`]), and
//!    the [`ModEnableList`] says which start disabled.
//! 4. Nothing is mounted yet: the game's entry script runs on the game's own files and loads its prefs. Then the enable list is
//!    read and the wanted mods' files are laid over the game's, later mounts over earlier ones ([`files`]).
//! 5. Then the [`ModStages`] the engine runs by itself run; a game runs the others with
//!    `loadMods("stage")` or [`Mods::load_stage`]. A stage runs each enabled mod's entry scripts as the mod's package, so
//!    disabling the mod deactivates it.
//!
//! Every rule in 1 to 3 is a slot a game or a plugin can fill ([`EngineMods`]), and the default of each is built on the same
//! traits. The limits and the path rules are applied below the layout, so no layout can load what they refuse.
//!
//! Scripts a server sends never run on a client: [`Mods::mount_server_files`] mounts them as content only.
//!
//! **Servers that require mods.** A game that opts in ([`ModsPlugin::required_mods`]) lets a server list mods its clients must
//! have: the handshake carries each required mod's id, version and a SHA-256 of its content, a client with another version or
//! hash is refused with the reason, and a server may send the content of a mod to a client that lacks it, over a bulk channel
//! into a per-user cache, checked against the hash before it is mounted. Only content is sent: no script, in any spelling,
//! is ever written, mounted or run from a server. See the guide's Mods chapter for the flow and [`content`] for the hash.
//!
//! # Maintaining
//!
//! **Mounts speak only for their own files.** `ModsPlugin` adds an asset classifier that says untrusted for the paths a mod or a
//! server mount serves and `None` for every other path; a path nobody says is trusted stays untrusted. Server files are checked with
//! `Assets::debug_assert_untrusted` when they are mounted (`mount_server_files`). A new kind of mount does both.
//!
//! Rules a change must keep, each guarded by a test:
//! - `read_prefix` answers as `read` does, cut: a mod's file over the game's, the files under it below
//!   and a zip mod's entry inflates only the prefix (`a_prefix_read_of_a_mods_file_is_the_start_of_what_a_read_gives`,
//!   `a_prefix_read_of_a_zip_mods_file_is_its_start`).

#![warn(missing_docs)]

mod cache;
pub mod content;
mod discover;
#[cfg(not(target_family = "wasm"))]
mod disk;
pub mod files;
mod info;
mod layout;
mod manifest;
mod mods;
mod natives;
pub mod offer;
mod package;
mod plan;
mod plugin;
mod share;
mod transfer;

#[cfg(not(target_family = "wasm"))]
pub use cache::DiskStore;
pub use cache::{
    CacheOwner, CacheScope, CacheStore, ContentCache, ENTRY_SUFFIX, IndexEntry, Listed,
    MAX_ENTRIES, MemoryStore, default_cache_dir, index_of,
};
pub use info::{
    Compare, MAX_LIST, MAX_NAME_LEN, MAX_TEXT_LEN, ModInfo, Requirement, Version, check_id,
};
pub use layout::{
    Action, Candidate, CandidateKind, DISABLED_PREF, ModEnableList, ModLayout, ModNames, ModOrder,
    ModPrereqs, ModRoot, ModStages, ModTomlLayout, NameInfo, OrderError, PlainNames,
    PrefsEnableList, PrereqPolicy, Problem, RootPlace, TopologicalOrder,
};
pub use manifest::{DEFAULT_FOLDER, MANIFEST, MAX_MANIFEST_BYTES};
pub use mods::{ModStatus, ModSummary, Mods, RequiredInfo, package_name};
pub use package::{MAX_PATH_LEN, ModError, ModLimits, ModPackage, RATIO_FLOOR};
pub use plan::ModSide;
pub use plugin::{EngineMods, ModsConfig, ModsPlugin, ModsRegistry};
pub use share::{ModShare, ModShareDefault, Share};
pub use transfer::{CHANNEL, CacheChoice, DownloadLimits, GET_RPC, ModsChannel, ModsGet};
