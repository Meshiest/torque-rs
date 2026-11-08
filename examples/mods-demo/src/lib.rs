//! A game mode written in Lua and shipped as a mod that the server requires.
//!
//! The game is a folder (`game/`): an entry script for the server and one for the client, a prefs file, and three mods in
//! `game/mods`:
//!
//! - `orbs`, the game mode: TorqueScript for its datablocks, Lua for its rules, and content (shapes, pictures, a sound) that
//!   [`assets`] makes from code. Its `mod.toml` says `share = "download"`, so a client that joins is sent the content, verifies
//!   it, mounts it and only then enters. The Lua runs on the server alone.
//! - `golden-orbs`, which requires `orbs` and so loads after it (though its name sorts first) and replaces two of its Lua
//!   functions. Players need nothing from it (`share = "none"`).
//! - `confetti`, which `game/prefs.cs` switches off: it is found and never mounted.
//!
//! The Lua extension is not linked in: it is built apart (`just lua`) and loaded from a `plugins/` folder, as a game loads it
//! ([`Lua`]). A [`Session`] is one server and its clients over the memory transport, stepped by hand; [`viewer`] draws a client
//! of one offscreen. `cargo run -p torque-example-mods-demo --bin mods-demo` plays a few rounds headless.

pub mod assets;
pub mod session;
pub mod viewer;

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::{DefaultPlugins, ModsPlugin};
use torque::app::server::{GameServer, Listen, ServerStart};
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginId};
use torque::extensions::{ApprovalRequest, NativeExtensions, NativeExtensionsPlugin};
use torque::mods::CacheChoice;
use torque_plugin_rope::RopePlugin;

pub use session::Session;

/// Where the server listens (on the memory transport: no socket is opened).
pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);

/// The server's entry function: makes the world the clients enter.
pub const START: &str = "startGame";

/// The manifest the Lua extension is loaded with (the one the extension's crate ships).
const LUA_MANIFEST: &str = include_str!("../../../crates/torque-plugin-lua/torque_lua.toml");

/// The game folder: this crate's `game/`, or in a packaged game (feature `bundled`) the `game/` beside the executable. A
/// `bundled` build holds no path of the computer it was built on, so run by cargo (tests with every feature on) it finds this
/// crate's folder from the variable cargo sets.
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

/// The server's files: the game folder (its scripts and its three mods) and the content the mods ship, made from code.
pub fn server_files() -> io::Result<GameFiles> {
    let mut files = GameFiles::default();
    files.add_dir(&game_dir(), "")?;
    for (id, content) in [("orbs", assets::orbs()?), ("confetti", assets::confetti()?)] {
        for (path, bytes) in content {
            files.insert(&format!("mods/{id}/{path}"), &bytes);
        }
    }
    Ok(files)
}

/// A client's files: the game and none of the mods, so the mods a server requires are the ones the client downloads.
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

/// The Lua extension on disk, ready to load: its library and manifest in a `plugins/` folder of its own.
#[derive(Clone, Debug)]
pub struct Lua {
    plugins: PathBuf,
    user: PathBuf,
}

/// The file name of the platform's library `name`.
pub fn library_file_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    }
}

impl Lua {
    /// Puts the built extension `library` (`just lua` makes it) and its manifest in a `plugins` folder under `dir` (emptied
    /// first), and keeps the extension's approvals in a `user` folder beside it.
    pub fn install(library: &Path, dir: &Path) -> io::Result<Lua> {
        let plugins = dir.join("plugins");
        let user = dir.join("user");
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(&plugins)?;
        std::fs::copy(library, plugins.join(library_file_name("torque_lua")))?;
        std::fs::write(plugins.join("torque_lua.toml"), LUA_MANIFEST)?;
        Ok(Lua { plugins, user })
    }

    /// The extension already in a `plugins` folder (a packaged game's), its approvals kept in `user`.
    pub fn at(plugins: &Path, user: &Path) -> Lua {
        Lua {
            plugins: plugins.to_path_buf(),
            user: user.to_path_buf(),
        }
    }

    /// The loader. A game asks the player before it loads a library the first time; this one holds only the library it was
    /// just handed, so it approves it.
    fn plugin(&self) -> NativeExtensions {
        NativeExtensions(
            NativeExtensionsPlugin::new(&self.plugins)
                .user_folder(&self.user)
                .approver(|_: &ApprovalRequest| true),
        )
    }
}

/// What an engine is for.
pub enum Role {
    /// The server: it requires its mods and serves them.
    Server,
    /// A client, whose downloads are kept in the folder `cache`.
    Client { cache: PathBuf },
}

/// An engine of the game for `role`: the default plugins, the rope plugin, and the Lua extension when there is one. Mods are
/// found in the game's files (`mods/`).
pub fn engine(files: &GameFiles, role: Role, lua: Option<&Lua>) -> Result<Engine, String> {
    let mods = match role {
        Role::Server => ModsPlugin::new().required_mods(),
        Role::Client { cache } => ModsPlugin::new().cache(CacheChoice::Folder(cache)),
    };
    let config = GameConfig::load(files).map_err(|e| e.to_string())?;
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((
            DefaultPlugins.set(mods),
            ExamplePlugin(files.clone()),
            RopePlugin::default(),
        ))
        .config(&config);
    if let Some(lua) = lua {
        builder
            .add_plugins(lua.plugin())
            .enable_id("torque.native-extensions");
    }
    builder.build().map_err(|e| e.to_string())
}

/// A server of the game on the memory transport, started by `engine`'s plugins, for a host that ticks it itself.
pub fn server(engine: &Engine) -> Result<GameServer, String> {
    let files = engine
        .resource::<GameFiles>()
        .ok_or("no ExamplePlugin")?
        .mem_files();
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SERVER),
            globals: &[],
            start: START,
            args: &[],
        },
    )
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

/// The built Lua extension's library: the file `TORQUE_LUA_LIBRARY` names, else the one `just lua` leaves in the build
/// folder (`release/`). In a packaged game (feature `bundled`) only the one in its `plugins/` folder.
pub fn lua_library() -> Option<PathBuf> {
    #[cfg(feature = "bundled")]
    return Some(
        bundle_dir()
            .join("plugins")
            .join(library_file_name("torque_lua")),
    )
    .filter(|library| library.is_file());
    #[cfg(not(feature = "bundled"))]
    dev_lua_library()
}

#[cfg(not(feature = "bundled"))]
fn dev_lua_library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TORQUE_LUA_LIBRARY") {
        return Some(PathBuf::from(path));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    let library = target.join("release").join(library_file_name("torque_lua"));
    library.is_file().then_some(library)
}
