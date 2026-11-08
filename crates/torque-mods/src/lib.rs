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

#![warn(missing_docs)]

mod discover;
#[cfg(not(target_family = "wasm"))]
mod disk;
pub mod files;
mod info;
mod layout;
mod manifest;
mod mods;
mod natives;
mod package;
mod plan;
mod plugin;

pub use info::{
    Compare, MAX_LIST, MAX_NAME_LEN, MAX_TEXT_LEN, ModInfo, Requirement, Version, check_id,
};
pub use layout::{
    Action, Candidate, CandidateKind, DISABLED_PREF, ModEnableList, ModLayout, ModNames, ModOrder,
    ModPrereqs, ModRoot, ModStages, ModTomlLayout, NameInfo, OrderError, PlainNames,
    PrefsEnableList, PrereqPolicy, Problem, RootPlace, TopologicalOrder,
};
pub use manifest::{DEFAULT_FOLDER, MANIFEST, MAX_MANIFEST_BYTES};
pub use mods::{ModStatus, ModSummary, Mods};
pub use package::{MAX_PATH_LEN, ModError, ModLimits, ModPackage, RATIO_FLOOR};
pub use plan::ModSide;
pub use plugin::{EngineMods, ModsConfig, ModsPlugin, ModsRegistry};
