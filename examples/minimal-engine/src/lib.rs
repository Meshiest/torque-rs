//! The minimal example again, built on [`Engine`] and [`DefaultPlugins`]: the same game as `examples/minimal` (its scripts and its
//! mission), a dedicated server and a client that connect over the loopback address, with the engine's helpers doing the wiring that
//! example does by hand.
//!
//! The game is one plugin, [`ExamplePlugin`]: its files, and the one global its client's GUI reads. The server, the console client and
//! the window client are each an engine of `(DefaultPlugins, ExamplePlugin)` run or started in a few lines (see `src/bin`). Which plugins
//! are on and which scripts the server and the client start with are not in the Rust: `game/game.toml` says (see [`builder`]).
//!
//! `cargo run -p torque-example-minimal-engine --bin min-engine-server`, and beside it `cargo run -p torque-example-minimal-engine
//! --bin min-engine-client` (or `cargo run -p torque-example-minimal-engine --features window --bin example-window-engine`, which with
//! `--local` runs the server itself).

use std::io;
use std::path::Path;

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::config::{EngineConfig, GameConfig};
use torque::app::files::GameFiles;
use torque::app::plugins::DefaultPlugins;
use torque::app::run::Dedicated;
use torque::app::server::{GameServer, Listen, ServerStart};
use torque::engine::{ClientStage, Engine, EngineBuilder, Plugin, PluginId};
use torque::ghost_world::world::Pose;

#[cfg(feature = "window")]
pub mod window;

/// The mission the server loads.
pub const MISSION: &str = "example/missions/tiny.mis";
/// The script function that loads it.
pub const START: &str = "loadExampleMission";
/// The most ghosts the client's GUI draws a box for.
pub const MAX_BOXES: usize = 64;

/// The example game's files: the minimal example's scripts and mission, this example's `server.cs` and `client.cs`, and the cube
/// (the asset kit's) as the shape of the crate and of the gem, which a real server loads and the hand-wired one never needs.
pub fn files() -> io::Result<GameFiles> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = GameFiles::load(&here.join("../minimal/game"))?;
    files.add_dir(&here.join("game"), "")?;
    let cube = torque::shapes::write::write_dts(&torque_example_assets::shapes::cube());
    files.insert("example/shapes/crate.dts", &cube);
    files.insert("example/shapes/gem.dts", &cube);
    Ok(files)
}

/// An engine builder for the game: `DefaultPlugins` and [`ExamplePlugin`], then `game/game.toml` applied (the plugins it turns
/// off, the scripts it names as the server's and the client's first). A game with no such file runs as it always did.
pub fn builder(files: &GameFiles) -> Result<EngineBuilder, String> {
    let config = GameConfig::load(files).map_err(|e| e.to_string())?;
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins, ExamplePlugin(files.clone())))
        .config(&config);
    Ok(builder)
}

/// The game: its files, which the server and the client read, and the number of boxes its client's GUI makes.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
pub struct ExamplePlugin(pub GameFiles);

impl Plugin for ExamplePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(self.0.clone());
        engine.client(ClientStage::Natives, |vm, _| {
            vm.set_global("$Example::Boxes", &MAX_BOXES.to_string());
            Ok(())
        });
    }
}

/// A dedicated server of the game on `listen`, for [`EngineDedicated::dedicated`](torque::app::run::EngineDedicated::dedicated).
pub fn dedicated(files: &GameFiles, listen: Listen) -> Dedicated {
    Dedicated {
        args: vec![MISSION.to_string()],
        ..Dedicated::new(files.mem_files(), listen, START)
    }
}

/// A server of the game on `listen`, started by the engine's plugins, for a game that ticks it itself.
pub fn server(engine: &Engine, listen: Listen) -> Result<GameServer, String> {
    let files = engine
        .resource::<GameFiles>()
        .ok_or("no ExamplePlugin")?
        .mem_files();
    let start = ServerStart {
        listen,
        globals: &[],
        start: START,
        args: &[MISSION],
    };
    GameServer::start_engine(engine, files, &start)
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

/// The class name and the position of each ghost the client holds that has one.
pub fn ghosts(client: &GameClient) -> Vec<(&'static str, [f32; 3])> {
    client
        .with_world(|world| {
            let mut found = Vec::new();
            for ghost in world.records.ghosts.values() {
                let position = match ghost.pose() {
                    Some(Pose::Affine { xf, .. }) => xf.position.map(|f| f.get()),
                    Some(Pose::Matrix { m, .. }) => [m[3], m[7], m[11]],
                    Some(
                        Pose::Euler { position, .. }
                        | Pose::Quat { position, .. }
                        | Pose::Item { position, .. },
                    ) => position,
                    None => continue,
                };
                found.push((ghost.class.map_or("placeholder", |c| c.name()), position));
            }
            found
        })
        .unwrap_or_default()
}
