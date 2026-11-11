//! torque-rs: a game engine library inspired by the Torque Game Engine and Torque3D, growing beyond their original
//! design.
//!
//! Games build on it by registering their own systems (console classes and functions, networked object types,
//! render passes, file formats and resource loaders) through the engine's registries, and change engine behaviour
//! only through those hooks.
//!
//! This crate gathers the engine crates in one dependency. Each area is a module behind a feature of the same name;
//! the default features are every area that needs no GPU, no window and no system library.
//!
//! | Module | Feature | Crate | What it is |
//! |---|---|---|---|
//! | `script` | `script` | `torque-ts-*` | TorqueScript: text rules, compiled scripts (DSO), the compiler and the runtime |
//! | `collide` | `collide` | `torque-collide` | convexes, GJK, polygon lists, working sets, object type masks |
//! | `engine` | `engine` | `torque-engine` | plugins, plugin groups and the engine builder that replays their recipes into each script engine |
//! | `content` | `content` | `torque-content` | the content formats as asset loaders: glTF, OBJ and Blockbench shapes, `.dif`, `.ter`, the map layer |
//! | `mods` | `mods` | `torque-mods` | local mods: folders and zips of scripts and assets mounted over a game's files, with replaceable rules for finding, ordering and enabling them |
//! | `shapes` | `shapes` | `torque-shapes` | DTS shapes and DSQ sequences, collision meshes, animated poses |
//! | `sim` | `sim` | `torque-sim` | the process list, players, AI players, items, projectiles, mounted images |
//! | `vehicle` | `vehicle` | `torque-vehicle` | rigid bodies, wheeled and flying vehicles |
//! | `mapcore`, `maps` | `maps` | `torque-mapcore`, `torque-maps` | the format-neutral map layer and Torque's map files |
//! | `net` | `net` | `torque-net` | connections, ghosting, events, bit streams, string compression |
//! | `ghosts` | `ghosts` | `torque-ghosts` | the networked objects' record codecs: TGE's ghost and datablock layouts, the class registry |
//! | `ghost_world` | `ghost-world` | `torque-ghost-world` | the client's world of ghosts: records merged, datablocks, images, threads, effects |
//! | `resource` | `resource` | `torque-resource` | a game folder with its zips mounted, served to the scripts as their files |
//! | `scene` | `scene` | `torque-scene` | the scene container's bins and searches, a scene object's transform maths |
//! | `game` | `game` | `torque-game` | the TGE game classes as console classes: scene objects, shapes, players, items, vehicles |
//! | `server` | `server` | `torque-server` | a dedicated server's pacing, platform values and datagram transport |
//! | `images` | `images` | `torque-images` | image decoding |
//! | `fonts` | `fonts` | `torque-fonts` | font caches, glyph metrics and text layout |
//! | `gui` | `gui` | `torque-gui` | the GUI object model, layout and the script methods of GUI classes |
//! | `audio` | `audio` | `torque-audio` | sound decoding, sources, channels and the mixer |
//! | `client` | `client` | `torque-client` | the client's engine half: the connection, prediction, action maps, the server query |
//! | `app_server` | `app-server` | `torque-app-server` | the server half of `app`: files, a server, terrain and water, the dedicated runner, `MinimalPlugins` (no GPU) |
//! | `app` | `app` (off by default) | `torque-app` | what a game made on the engine shares: files, server, client, window, world view, pictures |
//!
//! `app` is `app-core` with the engine's renderer (`renderer`); a game that draws only with renderers of its own takes
//! `app-core` and registers them on the renderer slot (see `torque::app::renderers`).
//!
//! Optional features that are off by default:
//!
//! - `audio-device`: sound output through the system's audio device (needs ALSA on Linux).
//! - `system-fonts`: the system's fonts as the fallback for a face with no font cache.
//! - `client-window`: the client's window glue (`client::platform`, over winit).
//! - `shape-builder`: `shape_builder`, a builder of shape models from code (`torque-shape-builder`).
//! - `full`: everything above.
//! - `native-extensions`: `extensions`, libraries loaded at run time from a local `plugins/` folder over the engine's C ABI
//!   (`torque-plugin-loader`); not in `full`, since an extension runs with the game's full rights.
//!
//! The crates that draw (the renderer and its back ends) need a GPU and join as features that are off by default:
//!
//! - `app`: [`app`], what a game made on the engine shares: its files, a server, a client with the server it hosts, a window,
//!   the world view under the GUI and offscreen pictures (over `torque-render-core`, `torque-render-hd`,
//!   `torque-render-scene` and winit; the `torque-app` crate).
//!
//! `torque::prelude` re-exports what a small game needs, so its `main.rs` has one import; `templates/game` in the repository is
//! such a game, the smallest one to copy.
//!
//! The examples in this crate's `examples/` folder run headless: `cargo run -p torque --example script` (also `shape`,
//! `collide` and `net`). The `torque-example-minimal` crate (`examples/minimal`) holds a dedicated server and a client built
//! on this crate, which connect over the loopback address (see the guide's "Server and client" chapter).

/// What a small game imports with one `use torque::prelude::*;`: the engine builder and plugins, a game's files and config,
/// the dedicated server's start-up, and with the `app` features the default plugins and the client. Each name is also at its
/// own path; a game that wants more reaches for those.
pub mod prelude {
    #[cfg(feature = "engine")]
    pub use crate::engine::{ClientStage, Engine, EngineBuilder, Plugin, PluginId};

    #[cfg(feature = "app-server")]
    pub use crate::app_server::config::{EngineConfig, GameConfig};
    #[cfg(feature = "app-server")]
    pub use crate::app_server::files::GameFiles;
    #[cfg(feature = "app-server")]
    pub use crate::app_server::run::{Dedicated, DedicatedRunnerPlugin, EngineDedicated};
    #[cfg(feature = "app-server")]
    pub use crate::app_server::server::{GameServer, Listen, ServerStart};

    #[cfg(feature = "app-core")]
    pub use crate::app::App;
    #[cfg(feature = "app-core")]
    pub use crate::app::client::{ClientBuilder, ClientNet, GameClient};
    #[cfg(feature = "app-core")]
    pub use crate::app::plugins::{DefaultPlugins, WindowRunnerPlugin};
    #[cfg(feature = "app-core")]
    pub use crate::app::run::EngineRun;
}

#[cfg(feature = "script")]
pub mod script {
    //! TorqueScript: the text and number rules ([`text`]), compiled scripts ([`bytecode`]), the compiler ([`compiler`])
    //! and the runtime with its console ([`vm`]).

    pub use torque_ts_bytecode as bytecode;
    pub use torque_ts_compiler as compiler;
    pub use torque_ts_text as text;
    pub use torque_ts_vm as vm;
}

#[cfg(feature = "app-core")]
pub use torque_app as app;
#[cfg(feature = "app-server")]
pub use torque_app_server as app_server;
#[cfg(feature = "audio")]
pub use torque_audio as audio;
#[cfg(feature = "client")]
pub use torque_client as client;
#[cfg(feature = "collide")]
pub use torque_collide as collide;
#[cfg(feature = "content")]
pub use torque_content as content;
#[cfg(feature = "engine")]
pub use torque_engine as engine;
/// Rust functions and impl blocks as TorqueScript natives: `#[torque::function(ns = "Pad")]`,
/// `#[torque::methods(class = "Player")]`, listed with `torque::natives![...]` for the engine's script registry (see
/// [`engine::function`] and [`engine::methods`]); `#[derive(torque::Prefs)]` for typed `$Pref::...` globals and
/// `#[derive(torque::EngineClass)]` for a console class with typed fields (see [`engine::Prefs`] and
/// [`engine::EngineClass`]); `#[derive(torque::GhostRecord)]` and `#[derive(torque::DatablockCodec)]` for a plugin's ghost
/// and datablock records (see [`engine::GhostRecord`] and [`engine::DatablockCodec`]).
#[cfg(feature = "engine")]
pub use torque_engine::{
    DatablockCodec, EngineClass, GhostRecord, Prefs, function, methods, natives,
};
#[cfg(feature = "fonts")]
pub use torque_fonts as fonts;
#[cfg(feature = "game")]
pub use torque_game as game;
#[cfg(feature = "ghost-world")]
pub use torque_ghost_world as ghost_world;
#[cfg(feature = "ghosts")]
pub use torque_ghosts as ghosts;
#[cfg(feature = "gui")]
pub use torque_gui as gui;
#[cfg(feature = "images")]
pub use torque_images as images;
#[cfg(any(feature = "maps", feature = "collide"))]
pub use torque_mapcore as mapcore;
#[cfg(feature = "maps")]
pub use torque_maps as maps;
#[cfg(feature = "mods")]
pub use torque_mods as mods;
#[cfg(feature = "net")]
pub use torque_net as net;
/// Native extensions: libraries loaded at run time over the C ABI (the `native-extensions` feature).
#[cfg(feature = "native-extensions")]
pub use torque_plugin_loader as extensions;
#[cfg(feature = "resource")]
pub use torque_resource as resource;
#[cfg(feature = "scene")]
pub use torque_scene as scene;
#[cfg(feature = "server")]
pub use torque_server as server;
#[cfg(feature = "shape-builder")]
pub use torque_shape_builder as shape_builder;
#[cfg(feature = "shapes")]
pub use torque_shapes as shapes;
#[cfg(feature = "sim")]
pub use torque_sim as sim;
#[cfg(feature = "vehicle")]
pub use torque_vehicle as vehicle;
