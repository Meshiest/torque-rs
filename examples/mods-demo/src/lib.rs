//! A game mode written in Lua and shipped as a mod that the server requires.
//!
//! The game is a folder (`game/`): an entry script for the server and one for the client, a prefs file, and three mods in
//! `game/mods`:
//!
//! - `soccer`, the game mode: TorqueScript for its datablocks, Lua for its rules, and content (shapes, pictures, a sound) that
//!   [`assets`] makes from code. Its `mod.toml` says `share = "download"`, so a client that joins is sent the content, verifies
//!   it, mounts it and only then enters. The Lua runs on the server alone.
//! - `big-ball`, which requires `soccer` and so loads after it (though its name sorts first), declares a bigger ball and
//!   replaces the Lua function that names the ball. Players need nothing from it (`share = "none"`).
//! - `confetti`, which `game/prefs.cs` switches off: it is found and never mounted.
//!
//! The ball is a prop of `torque-plugin-props`, which only the engine's own protocol carries, so every engine of the demo
//! speaks it whatever `TORQUE_NET_PROTOCOL` says. The Lua extension is not linked in: it is built apart (`just lua`) and
//! loaded from a `plugins/` folder, as a game loads it ([`Lua`]). A [`Session`] is one server and its clients over the memory
//! transport, stepped by hand, each client steered by a [`bot`]; [`viewer`] draws a client of one offscreen.
//! `cargo run -p torque-example-mods-demo --bin mods-demo` plays a few rounds headless.
//!
//! The game played by people: [`server`] starts the soccer server on UDP and the web, with bots in the places nobody has
//! taken (the mode's own, in Lua); [`game`] is the client a person plays, in a window or a browser (`web`): it shows the
//! mod's download, then the match from a camera beside the pitch ([`draw`] draws a client's world).
//!
//! # Maintaining
//!
//! The demo approves only the libraries it put in `plugins/` itself, by hash (a packaged game's are the hashes
//! `torque-bundle` built into the executable, `TORQUE_BUNDLE_PLUGINS`), and asks about any other on the terminal; a
//! packaged game keeps its approvals in the per-user folder (`a_bundle_ships_the_hashes_it_built_in`,
//! `only_a_shipped_library_is_approved_without_asking`, and with the built extension
//! `a_library_added_to_the_plugins_folder_is_not_approved_while_the_shipped_one_loads`).

pub mod assets;
#[cfg(not(target_arch = "wasm32"))]
pub mod bot;
pub mod draw;
pub mod game;
#[cfg(not(target_arch = "wasm32"))]
mod lua;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod session;
#[cfg(not(target_arch = "wasm32"))]
pub mod shots;
#[cfg(not(target_arch = "wasm32"))]
pub mod viewer;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::{DefaultPlugins, GuiPlugin, ModsPlugin, set_protocol};
#[cfg(not(target_arch = "wasm32"))]
use torque::app::server::{GameServer, Listen};
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginId, ProtocolKind};
use torque::mods::CacheChoice;
use torque_plugin_props::client::ClientProp;
use torque_plugin_props::{ClientProps, PropsPlugin};

use crate::game::JoinScreenPlugin;

#[cfg(not(target_arch = "wasm32"))]
pub use lua::{Lua, SHIPPED_PLUGINS, library_file_name, lua_library};
#[cfg(not(target_arch = "wasm32"))]
pub use session::Session;

/// Where the headless demo's server listens (on the memory transport: no socket is opened); a server people join takes
/// its port by default.
pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The server's entry function: makes the world the clients enter.
pub const START: &str = "startGame";

/// The pref that says how many bots may play (the soccer mod's `server.cs` gives it a default).
pub const BOTS_PREF: &str = "$Pref::Soccer::Bots";

/// The game folder: this crate's `game/`, or in a packaged game (feature `bundled`) the `game/` beside the executable. A
/// `bundled` build holds no path of the computer it was built on, so run by cargo (tests with every feature on) it finds this
/// crate's folder from the variable cargo sets.
#[cfg(not(target_arch = "wasm32"))]
fn game_dir() -> PathBuf {
    #[cfg(feature = "bundled")]
    {
        let beside = bundle_dir().join("game");
        match std::env::var_os("CARGO_MANIFEST_DIR") {
            Some(dir) if !beside.is_dir() => PathBuf::from(dir).join("game"),
            _ => beside,
        }
    }
    #[cfg(not(feature = "bundled"))]
    Path::new(env!("CARGO_MANIFEST_DIR")).join("game")
}

/// The folder of the running executable: a packaged game's root, which holds `game/` and `plugins/`.
#[cfg(feature = "bundled")]
pub fn bundle_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// The examples' shared script of hosting and joining (`examples/common/game/matches.cs`: the list of matches, the address box
/// and the download screen), built in so a packaged game carries it too.
pub const MATCHES_SCRIPT: (&str, &[u8]) = (
    "common/matches.cs",
    include_bytes!("../../common/game/matches.cs"),
);

/// The server's files: the game folder (its scripts and its three mods), the shared [`MATCHES_SCRIPT`] and the content the mods
/// ship, made from code.
#[cfg(not(target_arch = "wasm32"))]
pub fn server_files() -> io::Result<GameFiles> {
    let mut files = GameFiles::default();
    files.add_dir(&game_dir(), "")?;
    files.insert(MATCHES_SCRIPT.0, MATCHES_SCRIPT.1);
    for (id, content) in [
        ("soccer", assets::soccer()?),
        ("confetti", assets::confetti()?),
    ] {
        for (path, bytes) in content {
            files.insert(&format!("mods/{id}/{path}"), &bytes);
        }
    }
    Ok(files)
}

/// A client's files: the game and none of the mods, so the mods a server requires are the ones the client downloads.
#[cfg(not(target_arch = "wasm32"))]
pub fn client_files() -> io::Result<GameFiles> {
    let mut files = GameFiles::default();
    files.extend(
        server_files()?
            .iter()
            .filter(|(path, _)| !path.starts_with("mods/"))
            .map(|(path, bytes)| (path.to_string(), bytes.to_vec())),
    );
    Ok(files)
}

/// The game's plugin: its files, which the server and the client read.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
pub struct ExamplePlugin(pub GameFiles);

impl Plugin for ExamplePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(self.0.clone());
    }
}

/// What an engine is for.
pub enum Role {
    /// The server: it requires its mods and serves them.
    Server,
    /// A client, whose downloads are kept where `cache` says.
    Client { cache: CacheChoice },
}

/// An engine builder of the game whose mods plugin is `mods`: the default plugins in the engine's own protocol and the props
/// plugin. Mods are found in the game's files (`mods/`).
fn builder(files: &GameFiles, mods: ModsPlugin) -> Result<EngineBuilder, String> {
    let config = GameConfig::load(files).map_err(|e| e.to_string())?;
    let mut builder = EngineBuilder::new();
    // The GUI measures text with the fonts it draws with, so centered text is centered.
    builder.add_plugins(
        DefaultPlugins
            .set(mods)
            .set(GuiPlugin { layout_fonts: true }),
    );
    set_protocol(&mut builder, ProtocolKind::Engine);
    builder
        .add_plugins((PropsPlugin, ExamplePlugin(files.clone()), JoinScreenPlugin))
        .config(&config);
    Ok(builder)
}

/// An engine builder of a client of the game whose downloads are kept where `cache` says. A client has no Lua: the
/// server's Lua never runs on one.
pub fn client_builder(files: &GameFiles, cache: CacheChoice) -> Result<EngineBuilder, String> {
    builder(files, ModsPlugin::new().cache(cache))
}

/// [`client_builder`] for a client that names the server it joins (`server`, for its cache's keys) because its link's address
/// is the same for every server, as a browser's is.
pub fn named_client_builder(
    files: &GameFiles,
    cache: CacheChoice,
    server: &str,
) -> Result<EngineBuilder, String> {
    builder(files, ModsPlugin::new().cache(cache).cache_server(server))
}

/// An engine of the game for `role`, with the Lua extension when there is one.
#[cfg(not(target_arch = "wasm32"))]
pub fn engine(files: &GameFiles, role: Role, lua: Option<&Lua>) -> Result<Engine, String> {
    let mods = match role {
        Role::Server => ModsPlugin::new().required_mods(),
        Role::Client { cache } => ModsPlugin::new().cache(cache),
    };
    let mut builder = builder(files, mods)?;
    if let Some(lua) = lua {
        builder
            .add_plugins(lua.plugin())
            .enable_id("torque.native-extensions");
    }
    builder.build().map_err(|e| e.to_string())
}

/// The ball as the client of `engine` sees it: the newest prop its book holds (a new ball on the center spot replaces the old one).
pub fn ball_of(engine: &Engine) -> Option<ClientProp> {
    engine
        .resource::<ClientProps>()?
        .with(|book| book.props().values().next_back().copied())
        .flatten()
}

/// A server of the game on `listen`, started by `engine`'s plugins, for a host that ticks it itself: at most `bots` bots play
/// in the places nobody has taken.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_server(engine: &Engine, listen: Listen, bots: u32) -> Result<GameServer, String> {
    let options = server::ServerOptions {
        bots,
        dedicated: false,
        ..server::ServerOptions::default()
    };
    server::start(engine, &options, listen)
}

/// A client of the game with its socket as `net` says and its scripts run, not yet connected: `client.call("connectTo", ..)`.
pub fn client(engine: &Engine, net: ClientNet) -> Result<GameClient, String> {
    let files = engine
        .resource::<GameFiles>()
        .ok_or("no ExamplePlugin")?
        .mem_files();
    let mut builder = ClientBuilder::from_engine(engine, Some(files), net)?;
    builder.exec_entry()?;
    Ok(builder.build())
}
