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
//! **Client scripts.** A mod's manifest may ask for `[client_script]` (an `entry` Lua file of the mod's content and the `tiers` it
//! asks for, [`ClientScriptSpec`]). The server tells its clients ([`Mods::client_script_offers`], with the content hash the
//! clients verify), and a client's one opt-in reader of server-sent script files ([`Mods::client_scripts`],
//! [`Mods::client_script`], [`Mods::client_image`], and [`Mods::client_ready`], which reads nothing) gives them only to a sandboxed realm and only for the
//! mount that this server offered, that was verified against the hash, and that came by this server's own download or its own
//! cache entry. `exec`, `read_script` and the Lua extension's mod realms keep refusing every file a server sent.
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
//! - What the offer and the data message carry is versioned (`offer::WIRE_VERSION`, the digit in their head; the request has none).
//!   Change what they carry and the version moves with it; a message of another version is answered with `MessageError::Version` naming both
//!   (`a_message_of_another_version_is_refused_with_the_versions`). The content message the cache stores has its own head and is
//!   not part of this, so a cache survives it.
//! - A name from a server is shown on screens and read by scripts, so it is held to `modsync::is_display_name` (short, printable, no
//!   markup) when the offer is read, and the server sends the id for a mod whose name fails it
//!   (`a_display_name_that_is_too_long_or_not_plain_text_is_refused`, `a_mods_display_name_reaches_the_client_and_falls_back_to_the_id`
//!   in minimal-engine's tests).
//! - `read_prefix` answers as `read` does, cut: a mod's file over the game's, the files under it below
//!   and a zip mod's entry inflates only the prefix (`a_prefix_read_of_a_mods_file_is_the_start_of_what_a_read_gives`,
//!   `a_prefix_read_of_a_zip_mods_file_is_its_start`).
//! - A server's offer costs the client no more than it downloads: ids come once and only as a local mod's could be
//!   (`an_offer_naming_one_id_twice_or_an_id_a_local_mod_could_not_have_is_refused`), the cache's heads are read once an
//!   offer and older copies only up to twice the offer's download (`an_offer_loads_each_ids_older_copies_once_and_within_a_budget`).
//! - Client scripts are read from one kind of mount: the one a server offered on this connection, verified against the offered
//!   hash, from its own download or its own cache entry. A mod the player installed, content another server sent, an entry found
//!   through cache sharing, a mount the public `mount_server_files` made and a mount another server's connection replaced give
//!   nothing (`client_script_content_is_read_only_for_the_server_that_sent_it`, `client_script_content_found_through_cache_sharing_is_refused`,
//!   `client_script_content_from_a_mount_with_no_origin_is_never_read`, `client_script_content_is_gone_when_another_server_replaces_the_mount`,
//!   `client_script_content_with_more_files_than_the_realm_takes_is_refused`), and asking whether it is there reads no bytes
//!   (`client_script_ready_looks_without_reading_and_follows_the_same_rules`). The manifest's request is checked and an entry that
//!   is not content (a stage script, a path that escapes) asks for nothing (`client_script_the_manifest_asks_for_scripts_and_a_bad_request_is_a_warning`,
//!   `client_script_a_required_mod_is_offered_with_its_entry_tiers_and_content_hash`).
//! - One server cannot flush the others' cache (`one_server_storing_new_content_cannot_flush_another_servers_entries`), the
//!   disk cache opens nothing through a link (`a_link_in_the_cache_folder_is_never_read_or_touched_through`), and a server's
//!   file is never copied to a script's name (`server_sent_script_never_executes_on_a_client`).
//! - A download asks for `DOWNLOAD_WINDOW` (three) runs at once. The server limits what a client has queued on the mods channel
//!   alone (not the other channels') to the window and one run (`mods::MAX_OUTSTANDING`), and the client takes the answers in
//!   the order it asked; a run answered out of turn ends the download
//!   (`a_download_takes_a_fraction_of_the_round_trips_a_request_at_a_time_would`,
//!   `a_download_of_many_runs_completes_with_another_channels_backlog_beside_it` in minimal-engine's tests).
//! - The browser's cache is a [`MirroredStore`] over IndexedDB (`web_cache`): the cache runs on memory, each change goes to the
//!   database in a transaction of its own (the browser runs them in order), and what is read back at the page's start is held to
//!   the entry names and the store's own cap (`what_is_not_an_entry_or_does_not_fit_is_removed_from_the_database_on_load`,
//!   `a_store_that_holds_less_than_the_cap_asks_lowers_it`) and to the hash check of every read, which deletes an entry whose
//!   bytes changed here and in the database (`a_changed_entry_is_dropped_from_memory_and_the_database_and_downloaded_again`).
//!   Entries are keyed by the name the client gives its server (`ModsPlugin::cache_server`), because a browser's link dials one address
//!   for every server, and the browser's storage is used only with a name
//!   (`two_servers_behind_one_address_cannot_see_each_others_entries_even_after_a_reload`). The database is read one value at a time
//!   with the length checked before the copy, so a tampered database cannot run the page out of memory. A database that is missing or
//!   refuses a write only leaves the cache in memory
//!   (`a_database_that_refuses_writes_leaves_a_cache_that_works_for_the_session`). A new kind of backing implements [`Backing`] and
//!   takes the same tests; `web/soccer-check.mjs` runs the real one (a reload with nothing to download, a changed entry downloaded
//!   again).
//! - A scan of the mods folder visits a bounded number of entries of every kind and keeps at most 64 warnings
//!   (`a_mod_of_many_empty_folders_is_skipped_quickly_with_one_warning`, `the_warnings_of_a_scan_are_capped`); download
//!   places are shared out per network (`one_network_holds_at_most_two_download_places` in minimal-engine's tests).

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
mod mirror;
mod mods;
mod natives;
pub mod offer;
mod package;
mod plan;
mod plugin;
mod share;
mod transfer;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod web_cache;

#[cfg(not(target_family = "wasm"))]
pub use cache::DiskStore;
pub use cache::{
    CacheOwner, CacheScope, CacheStore, ContentCache, ENTRY_SUFFIX, IndexEntry, Listed,
    MAX_ENTRIES, MemoryStore, default_cache_dir, index_of, server_name_of_room, server_name_of_url,
};
pub use files::{MAX_CLIENT_FILES, MAX_CLIENT_IMAGE_BYTES, MAX_CLIENT_SCRIPT_BYTES};
pub use info::{
    ClientScriptSpec, Compare, MAX_CLIENT_TIERS, MAX_LIST, MAX_NAME_LEN, MAX_TEXT_LEN, ModInfo,
    Requirement, Version, check_id,
};
pub use layout::{
    Action, Candidate, CandidateKind, DISABLED_PREF, ModEnableList, ModLayout, ModNames, ModOrder,
    ModPrereqs, ModRoot, ModStages, ModTomlLayout, NameInfo, OrderError, PlainNames,
    PrefsEnableList, PrereqPolicy, Problem, RootPlace, TopologicalOrder,
};
pub use manifest::{DEFAULT_FOLDER, MANIFEST, MAX_MANIFEST_BYTES};
pub use mirror::{Backing, MirroredStore, Record};
pub use mods::{ClientScriptOffer, ModStatus, ModSummary, Mods, RequiredInfo, package_name};
pub use package::{MAX_PATH_LEN, ModError, ModLimits, ModPackage, RATIO_FLOOR};
pub use plan::ModSide;
pub use plugin::{EngineMods, ModsConfig, ModsPlugin, ModsRegistry};
pub use share::{ModShare, ModShareDefault, Share};
pub use transfer::{CHANNEL, CacheChoice, DownloadLimits, GET_RPC, ModsChannel, ModsGet};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use web_cache::{WEB_CACHE_BYTES, open_browser_cache};
