//! A singleplayer world: a menu of cube maps, then the asset kit's box figure walking and jumping on the map's cubes, a server and
//! a client of the engine joined in one process.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`ground`]: a map's terrain and water as the client draws them (the valley's).
//! - [`levels`]: maps authored as Blockbench models, built into interiors when the example starts.
//! - [`plugin`]: the game as an engine plugin ([`plugin::LevelPlugin`]) and the engine it runs on.
//! - [`props`]: a level's furniture and desk items, built into props a figure pushes.
//! - `ropes` (the `rope` feature): the plaza's clothesline, made by the rope plugin.
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on an in-memory transport.
//! - `catalog` (native builds): the office's prop catalog, a mod the server requires of its clients whose menu runs in a client-script realm
//!   on each (see `catalog/` and the book's server scripts chapter).
//! - [`office`]: the office played with others: its server on UDP and the web, the engine its clients are made by, and a client's
//!   server scripts.
//! - `session` (native builds): the office played with others on this computer, stepped by hand (the multiplayer tests and
//!   pictures).
//! - [`client`]: the client host, the server of the map being played, the link between them, and a frame for the world view.
//! - [`scene`]: the client's ghosts as the world view's objects (the cubes and the figures).
//! - [`showcase`]: pictures of the world alone at the highest quality: the valley's views and the front page's pictures.
//! - [`shots`]: the example without a window, drawn offscreen: pictures of the menu and of every map.
//! - `web` (built for the web only): the example in a browser, its files fetched by the page or carried inside it.
//!
//! The window is the engine's ([`torque::app::window::run`]).
//!
//! # Maintaining
//!
//! **Flow.** [`plugin::LevelPlugin::new`] builds the files before any engine runs: each level ([`levels::add_levels`]: its
//! model cut into its props and its building, [`props::split`]; the props' shapes and datablocks, [`props::build`]; the
//! building's interiors) and the valley. [`plugin::level_engine`] adds the engine's plugins, the engine's own protocol, the
//! props and their prediction; [`client::LevelDemo`] starts a map's server and joins it over the memory link.
//!
//! **With others.** [`office::office_engine`] adds the list of matches, the mods (the catalog required of a server's players)
//! and server scripts; the window's client is [`client::LevelDemo::for_matches`] over it, on a UDP socket (a page's game socket
//! in a browser), and plays the maps alone with [`torque::app::client::GameClient::start_local`]. Hosting and joining are the
//! examples' `common/matches.cs` screens; the host's side is `$Game::HostAsked` and `$World::Joining`. A client's realm
//! ([`office::ServerScripts`]) is framed only while the player is in the world, so the catalog opens over the office and not
//! over the join screen, which shows no server controls.
//!
//! **Profiles.** `level-profile` times the office's and the valley's views offscreen (under the bench lock); `--gi` turns the
//! renderer's global illumination on and waits for its voxels before timing.
//!
//! **Invariants**, each with the test that guards it (`tests/office.rs` unless named):
//! - The office's building is one interior that passes the builder's verification:
//!   `the_office_builds_into_one_interior_that_passes_verification`.
//! - Every movable piece of furniture and every desk item is a prop, and the building keeps only its fittings:
//!   `every_piece_of_furniture_and_every_desk_item_is_a_prop_and_the_fittings_stay_interior` (with the props' caps).
//! - The props start asleep where the model put them and nothing moves untouched; a woken one rests where it was:
//!   `the_props_start_asleep_where_the_model_put_them_and_nothing_moves_while_no_one_touches_them`,
//!   `every_prop_rests_on_the_floor_a_rug_or_the_piece_under_it_when_woken`.
//! - The figure pushes what it walks into and is stopped by the fittings: `the_figure_pushes_the_chair_it_walks_into`,
//!   `the_figure_walks_in_through_the_doors_stopped_by_the_fittings_or_pushing_the_furniture`.
//! - A click picks up the light prop the figure looks at, in reach and in sight, which it carries within a few centimeters of
//!   its hand and its client predicts; a heavy, far or held one is refused; the holder dying drops it:
//!   `a_click_picks_up_the_chair_it_looks_at_carries_it_on_a_walk_and_a_second_click_drops_it`,
//!   `a_prop_out_of_reach_too_heavy_or_held_by_another_is_refused`, `the_holder_dying_drops_what_it_holds` (the rules are
//!   `game/server.cs`'s `grabFor`; the hold is the props plugin's).
//! - The props are drawn as the interior is (nearest texels, `_glow` unlit):
//!   `the_props_shapes_are_drawn_nearest_and_their_glow_unlit`, and the look in `--office-shots`.
//! - What the scripts can call: `tests/natives.rs` against `tests/natives/` (`TORQUE_BLESS=1` rewrites them).
//! - Played with others (`tests/multiplayer.rs`, with the realm guest): a dedicated office server on UDP is found in the list
//!   of matches, its catalog downloads on the join screen, the player allows it, puts a prop down that another player's client
//!   gets and picks up (`a_player_finds_the_office_downloads_the_catalog_allows_it_and_puts_down_a_prop_another_picks_up`); a
//!   second join downloads nothing, the answer is remembered, and a map played alone after it offers no server scripts
//!   (`leaving_and_joining_again_downloads_nothing_and_the_answer_is_remembered`). The browser's join over WebSocket, a data
//!   channel and a room is `web/office-check.mjs` (`just examples level-demo office-web-check`).
//!
//! **Extension points.** A level is `game/levels/<name>.bbmodel` beside `game/levels/<name>.mis` (its `%interiors%` line,
//! and a `%props%` line when the model has prop groups); a kind of prop is a [`props::PropKind`] and its mass. The office is
//! made by `tools/office.py` (`python -I tools/office.py`), which marks each item as its own prop group.
//!
//! **Checks.** `cargo test -p torque-example-level-demo` (and with `--features rope`), its clippy with `-D warnings`, and
//! `cargo run -p torque-example-level-demo --release -- --office-shots <dir>` for the look.

use torque::sim::player::PoseData;

#[cfg(not(target_arch = "wasm32"))]
pub mod catalog;
pub mod client;
pub mod files;
pub mod ground;
#[cfg(not(target_arch = "wasm32"))]
pub mod icons;
pub mod levels;
pub mod office;
pub mod plugin;
pub mod props;
#[cfg(feature = "rope")]
pub mod ropes;
pub mod scene;
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod session;
pub mod shots;
pub mod showcase;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// The figure's `swimBoundingBox` and `swimForce` (`game/server.cs`).
pub const SWIM_BOX: [f32; 3] = [1.0, 2.3, 1.8];
pub const SWIM_FORCE: f32 = 2400.0;

/// The figure's poses as the level demo's server gives them: the examples' ([`torque_example_common::figure_poses`]), and
/// swimming. A client cannot read a datablock's dynamic fields, so the client's prediction and the drawing of the other
/// figures take them from here; a test holds the two the same.
pub fn figure_poses() -> PoseData {
    PoseData {
        swim: true,
        swim_box: SWIM_BOX,
        swim_force: SWIM_FORCE,
        ..torque_example_common::figure_poses()
    }
}
