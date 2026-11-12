//! A real server and engine clients in one process over the memory transport, stepped by hand: one 32 ms tick of the server, then
//! one of each client, the datagrams carried across around each. No clock is read.

// Each test file uses some of what is here.
#![allow(dead_code)]

use torque_app::GameFiles;
use torque_app::client::GameClient;
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
#[allow(unused_imports)]
pub use torque_app::testing::Link;
pub use torque_app::testing::Peer;
use torque_app::testing::RigPlugin;
use torque_engine::{Engine, EngineBuilder, Plugin};
use torque_net::connect::SharedSecret;
use torque_plugin_props::{ClientProps, PropsPlugin, ServerProps};

/// The props plugin as the rig's subject.
pub struct Props;

impl RigPlugin for Props {
    const START: &'static str = "startProps";

    fn add(builder: &mut EngineBuilder) {
        builder.add_plugins(PropsPlugin);
    }

    /// The game's files for the shapes the client's prediction reads (the floor it stands on), as a game gives them.
    fn client(client: &mut GameClient, files: &GameFiles) {
        if torque_client::game::installed_game(client.vm()).is_none() {
            torque_client::game::set_client_game(
                client.vm_mut(),
                std::rc::Rc::new(FilesGame(files.mem_files())),
            );
        }
    }
}

/// A server of the props engine and its clients.
pub type Rig = torque_app::testing::Rig<Props>;

pub fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("server.cs"));
    files.insert("main.cs", include_bytes!("main.cs"));
    files.insert("game.toml", include_bytes!("game.toml"));
    let dts = torque_shapes::write::write_dts;
    files.insert("kit/cube.dts", &dts(&torque_example_assets::shapes::cube()));
    files.insert(
        "kit/figure.dts",
        &dts(&torque_example_assets::shapes::figure()),
    );
    files.insert(
        "kit/buggy.dts",
        &dts(&torque_example_assets::buggy::buggy()),
    );
    files.insert(
        "kit/buggy_wheel.dts",
        &dts(&torque_example_assets::buggy::wheel()),
    );
    files
}

/// An engine of the default plugins (the game's `game.toml` applied) and, when `props` is true, the props plugin.
pub fn engine(files: &GameFiles, props: bool) -> Engine {
    if props {
        torque_app::testing::engine::<Props>(files)
    } else {
        torque_app::testing::plain_engine(files)
    }
}

/// The secret the test accounts sign in under.
const SECRET: [u8; 16] = [3; 16];

/// Sets the auth provider: tickets signed under [`SECRET`] for `name` (a server's half takes an empty name).
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.sign-in")]
struct SignIn(&'static str);

impl Plugin for SignIn {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.net().auth(SharedSecret::new(SECRET, self.0));
    }
}

/// [`engine`] (with props) and a real auth provider: a client that joins with it is the account `name`, whatever its
/// connection, and a server (empty `name`) holds every account to the same secret.
pub fn signed_in_engine(files: &GameFiles, name: &'static str) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(DefaultPlugins)
        .config(&config)
        .add_plugins((PropsPlugin, SignIn(name)));
    builder.build().expect("the engine builds")
}

/// [`engine`] with the engine's own protocol pinned, whatever the environment says: what the scene hierarchy needs.
pub fn engine_pinned(files: &GameFiles) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    torque_app_server::plugins::set_protocol(&mut builder, torque_engine::ProtocolKind::Engine);
    builder.add_plugins(PropsPlugin);
    builder.build().expect("the engine builds")
}

/// Torque's answers, with the game's files for the shapes the client reads.
#[derive(Debug)]
struct FilesGame(torque_ts_vm::host::MemFiles);

impl torque_client::game::ClientGame for FilesGame {
    fn file_reader(&self) -> Option<torque_client::predict::FileReader> {
        let files = self.0.clone();
        Some(std::rc::Rc::new(
            move |path: &[u8]| match torque_ts_vm::host::FileHost::read(&files, path) {
                torque_ts_vm::host::FileRead::Found(bytes) => Some(bytes),
                _ => None,
            },
        ))
    }
}

/// What a test reads off a client of the props engine.
pub trait PeerProps {
    /// The client's book of props.
    fn book(&self) -> ClientProps;
    /// The client's props by the server's slot: (slot, position, asleep).
    fn props(&self) -> Vec<(u16, [f32; 3], bool)>;
}

impl PeerProps for Peer {
    fn book(&self) -> ClientProps {
        self.engine
            .resource::<ClientProps>()
            .expect("the book")
            .clone()
    }

    fn props(&self) -> Vec<(u16, [f32; 3], bool)> {
        let book = self.book();
        let b = book.0.borrow();
        let mut out: Vec<_> = b
            .props()
            .values()
            .map(|p| (p.slot, p.q.position(), p.asleep))
            .collect();
        out.sort_by_key(|p| p.0);
        out
    }
}

/// What a test reads off the server of the props engine.
pub trait RigProps {
    /// The server's book of props.
    fn props(&self) -> ServerProps;
    /// The server's props by slot: (slot, position, asleep).
    fn server_props(&self) -> Vec<(u16, [f32; 3], bool)>;
    /// The ghost section totals of each client's connection, in join order.
    fn totals(&self) -> Vec<torque_server::net::ghosts::Totals>;
}

impl RigProps for Rig {
    fn props(&self) -> ServerProps {
        self.engine
            .resource::<ServerProps>()
            .expect("the server's props")
            .clone()
    }

    fn server_props(&self) -> Vec<(u16, [f32; 3], bool)> {
        let props = self.props();
        let s = props.0.borrow();
        let mut out: Vec<_> = s
            .world
            .as_ref()
            .map(|w| {
                w.props()
                    .values()
                    .map(|p| (p.slot, p.q.position(), p.rest.is_asleep()))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|p| p.0);
        out
    }

    fn totals(&self) -> Vec<torque_server::net::ghosts::Totals> {
        let shared = self.server.shared().expect("the network host").borrow();
        let hub = shared.ghosts.clone().expect("a ghost hub");
        let mut conns: Vec<_> = shared.conns.values().filter_map(|c| c.conn).collect();
        conns.sort_unstable();
        conns.iter().filter_map(|&c| hub.totals(c)).collect()
    }
}

/// A server and `n` clients of the props engine, connected and ghosted; `prefs` set on each client.
pub fn clients(n: usize, globals: &[(&str, &str)], prefs: &[(&str, &str)]) -> (Rig, GameFiles) {
    let files = files();
    let mut rig = Rig::new(&files, globals);
    for k in 0..n {
        rig.join(&files, engine(&files, true), 50_001 + k as u16, prefs);
    }
    assert!(
        rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 2)),
        "{:?}",
        rig.peers[0].client.client_log
    );
    (rig, files)
}
