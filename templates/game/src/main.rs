//! The smallest game: `cargo run` opens a window joined to a server it runs itself. `cargo run -- server [port]` is a dedicated
//! server and `cargo run -- client [address]` a client with no window that joins one. The scripts are in `game/`. It speaks
//! the engine's own protocol over the QUIC link (`DefaultPlugins`); `[net] protocol = "tge"` in `game/game.toml` makes it speak
//! Torque's over plain UDP.

mod window;

use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use torque::prelude::*;

/// The script function that starts the server's game.
const START: &str = "startGame";

/// The game's own plugin: the game's name, from `game/game.toml`, as a global its scripts read. Plugins are how a game adds its
/// natives, classes and settings.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct GamePlugin(String);

impl Plugin for GamePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let name = self.0.clone();
        engine.client(ClientStage::Natives, move |vm, _| {
            vm.set_global("$Game::Name", &name);
            Ok(())
        });
    }
}

/// The engine of the game: the default plugins and `GamePlugin`, set up as `game/game.toml` says.
fn builder(files: &GameFiles) -> Result<EngineBuilder, Box<dyn Error>> {
    let config = GameConfig::load(files)?;
    let name = config.name.clone().unwrap_or_default();
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins, GamePlugin(name)))
        .config(&config);
    Ok(builder)
}

/// A server that has run `server.cs` and called `startGame()`, listening on `listen`.
fn server(engine: &Engine, files: &GameFiles, listen: Listen) -> Result<GameServer, String> {
    let start = ServerStart {
        listen,
        globals: &[],
        start: START,
        args: &[],
    };
    GameServer::start_engine(engine, files.mem_files(), &start)
}

/// A client that has run `client.cs`, not yet connected: `client.call("connectTo", ..)`.
fn client(engine: &Engine, files: &GameFiles, net: ClientNet) -> Result<GameClient, String> {
    let mut client = ClientBuilder::from_engine(engine, Some(files.mem_files()), net)?;
    client.exec_entry()?;
    Ok(client.build())
}

/// A client with no window, connecting to `address` over the loopback network.
fn join(engine: &Engine, files: &GameFiles, address: SocketAddr) -> Result<GameClient, String> {
    let mut client = client(engine, files, ClientNet::Loopback)?;
    client.call("connectTo", &[&address.to_string()])?;
    Ok(client)
}

fn main() -> Result<(), Box<dyn Error>> {
    let files = GameFiles::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("game"))?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("server") => {
            let port = args.next().map_or(Ok(28000), |port| port.parse())?;
            let listen = Listen::Udp {
                address: Ipv4Addr::LOCALHOST,
                port,
                public: false,
            };
            builder(&files)?
                .disable::<WindowRunnerPlugin>()
                .add_plugins(DedicatedRunnerPlugin)
                .dedicated(Dedicated::new(files.mem_files(), listen, START))
                .run()?;
        }
        Some("client") => {
            let address = args.next().unwrap_or_else(|| "127.0.0.1:28000".into());
            let engine = builder(&files)?.build()?;
            let mut client = join(&engine, &files, address.parse()?)?;
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                client.step(5);
                std::thread::sleep(Duration::from_millis(5));
            }
            client.call("disconnect", &[])?;
            println!("{}", client.client_log.join("\n"));
        }
        None => {
            let game = files.clone();
            builder(&files)?
                .app(move |engine| window::Window::new(engine, &game))
                .run()?;
        }
        Some(_) => return Err("usage: cargo run [-- server [port] | client [address]]".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks `server` and steps `client` until the server prints `line`.
    fn until_server_says(line: &str, server: &mut GameServer, client: &mut GameClient) {
        let start = Instant::now();
        let mut log = Vec::new();
        while !log.iter().any(|l: &String| l.contains(line)) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the server never said {line:?}: {log:?}"
            );
            server.tick();
            client.step(32);
            log.extend(server.drain_log());
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    #[test]
    fn a_client_joins_the_server_and_leaves() {
        let files = GameFiles::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("game")).unwrap();
        let engine = builder(&files).unwrap().build().unwrap();
        let listen = Listen::Udp {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            public: false,
        };
        let mut server = server(&engine, &files, listen).unwrap();
        let mut client = join(&engine, &files, server.address().unwrap()).unwrap();
        let name = GameConfig::load(&files).unwrap().name.unwrap();

        until_server_says("Player joined", &mut server, &mut client);
        let said = format!("Playing {name}");
        assert!(
            client.client_log.iter().any(|l| l.contains(&said)),
            "{:?}",
            client.client_log
        );
        client.call("disconnect", &[]).unwrap();
        until_server_says("Player left", &mut server, &mut client);
    }
}
