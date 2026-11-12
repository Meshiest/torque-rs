//! The game a person plays: a client of a soccer server, in a window or on a browser's page, that joins the way the arena
//! does (`examples/fps`): a main menu, a match hosted here or one from the network's list of games or typed by address, the
//! screen that shows the server's mods downloading, then the match. Its scripts are `game/client.cs`.
//!
//! **The socket** ([`NetMode`], the engine's [`torque::app::hosting`]): [`NetMode::Loopback`] reads only loopback datagrams
//! (tests), [`NetMode::Lan`] binds as TGE binds one and reaches the network and one public address the player typed. A
//! browser's client has a game socket of its own instead ([`SoccerGame::from_engine`] with a socket).
//!
//! **Hosting** (`$Game::HostAsked`, natively): the server ([`crate::server`], with its Lua) runs in this process on the LAN
//! address (or every interface for a public match) and the client joins it over the loopback address. The client has none of
//! the mods, so it downloads the soccer mod from its own server the first time, as anyone else does.
//!
//! **The camera** ([`pitch_camera`]): fixed, the whole pitch from its side with the player's team attacking to the right, so
//! the ball, both goals and every player stay in view and the keys move the player across the screen. Before the client has a
//! team it looks on from Blue's side.

use std::net::Ipv4Addr;

use torque::app::client::{ClientBuilder, GameClient};
use torque::app::files::GameFiles;
pub use torque::app::hosting::NetMode;
use torque::app::hosting::register_join_natives;
use torque::app::plugins::ClientWiring;
use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::client::net::{Datagrams, NetPolicy};
use torque::engine::{ClientStage, Engine, EngineBuilder, Plugin, PluginId};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::draw::{Drawing, world_view};

/// The UDP port number servers listen on by default, and the one the list of games asks on.
pub const DEFAULT_PORT: u16 = 28_000;

/// The fixed camera beside the pitch for a player attacking toward `attack` along y (1: +y, -1: -y; 0 before it has a team):
/// from the side and above, the whole pitch in view, that team attacking to the right.
#[must_use]
pub fn pitch_camera(attack: f32) -> Eye {
    let side = if attack < 0.0 { -1.0 } else { 1.0 };
    look_at([15.0 * side, 0.0, 13.5], [0.5 * side, 0.0, -0.5])
}

/// The join screen's natives (`checkJoinAddress` and the rest): what the menu's address field asks.
#[derive(Clone, Copy, Default, PluginId)]
#[plugin(crate = torque::engine)]
pub struct JoinScreenPlugin;

impl Plugin for JoinScreenPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            let wiring = vm
                .extension::<ClientWiring>()
                .ok_or("the join screen runs only when a game client is made")?;
            register_join_natives(vm, wiring.state());
            Ok(())
        });
    }
}

/// Where a join stands: what the join screen shows (`$Join::State` in `game/client.cs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JoinState {
    /// At a menu, in no server.
    Menu,
    /// Asking the server to let the client in.
    Connecting,
    /// Let in; the server has not said which mods it plays yet.
    Accepted,
    /// The server's mods coming in.
    Downloading,
    /// The server's mods were all here already: nothing to download.
    Cached,
    /// Downloaded; waiting for the match.
    Joining,
    /// In the match.
    Playing,
    /// The join ended: the screen says why.
    Failed,
}

impl JoinState {
    /// The state the scripts' word names (none, or a word they never set: [`JoinState::Menu`]).
    #[must_use]
    pub fn from_script(word: &str) -> JoinState {
        match word {
            "connecting" => JoinState::Connecting,
            "accepted" => JoinState::Accepted,
            "downloading" => JoinState::Downloading,
            "cached" => JoinState::Cached,
            "joining" => JoinState::Joining,
            "playing" => JoinState::Playing,
            "failed" => JoinState::Failed,
            _ => JoinState::Menu,
        }
    }
}

/// How a client is made.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub net: NetMode,
    /// The player's name (the menu's field starts with it).
    pub name: String,
    /// The UDP port number the list of games asks on and a hosted match binds.
    pub port: u16,
    /// The address a hosted match binds (loopback: this computer's LAN address is found).
    pub host_address: Ipv4Addr,
    /// The Lua extension a hosted match's server runs its game mode with (none: hosting says it needs it).
    #[cfg(not(target_arch = "wasm32"))]
    pub lua: Option<crate::Lua>,
}

impl Default for ClientOptions {
    fn default() -> ClientOptions {
        ClientOptions {
            net: NetMode::Lan,
            name: "Player".to_string(),
            port: DEFAULT_PORT,
            host_address: Ipv4Addr::LOCALHOST,
            #[cfg(not(target_arch = "wasm32"))]
            lua: None,
        }
    }
}

/// The game: its client and what draws it.
pub struct SoccerGame {
    pub client: GameClient,
    pub options: ClientOptions,
    drawing: Drawing,
    /// The server's engine, made the first time a match is hosted here.
    #[cfg(not(target_arch = "wasm32"))]
    server_engine: Option<Engine>,
    /// The seconds the last step covered, for the next frame.
    dt: f32,
}

impl SoccerGame {
    /// The client made by `engine` (the game's client engine: [`crate::client_builder`], built) over `files`, on `socket` if
    /// it has one (a browser's game socket; its address rule is the loopback one), else on a socket as `options.net` says.
    /// Its scripts have run: the main menu shows.
    pub fn from_engine(
        engine: &Engine,
        files: &GameFiles,
        options: ClientOptions,
        socket: Option<Box<dyn Datagrams>>,
    ) -> Result<SoccerGame, String> {
        let net = if socket.is_some() {
            torque::app::client::ClientNet::External
        } else {
            options.net.client_net()
        };
        let mut b = ClientBuilder::from_engine(engine, Some(files.mem_files()), net)?;
        if let Some(socket) = socket {
            b.set_socket(socket, NetPolicy::Loopback);
        }
        b.vm.set_global("$Pref::Player::Name", &options.name);
        b.vm.set_global("$Game::Port", &options.port.to_string());
        b.exec_entry()?;
        let mut client = b.build();
        let drawing = Drawing::new(&mut client)?;
        Ok(SoccerGame {
            client,
            options,
            drawing,
            #[cfg(not(target_arch = "wasm32"))]
            server_engine: None,
            dt: 0.016,
        })
    }

    /// Joins the server at `address` (`a.b.c.d:port`).
    pub fn join(&mut self, address: &str) -> Result<(), String> {
        self.client.call("connectTo", &[address]).map(drop)
    }

    /// Where the join stands, as the scripts keep it (`$Join::State`).
    pub fn join_state(&self) -> JoinState {
        JoinState::from_script(&self.client.global("$Join::State"))
    }

    /// Whether the client is in the match.
    pub fn playing(&self) -> bool {
        self.join_state() == JoinState::Playing
    }

    /// The eye the frame is drawn from: [`pitch_camera`] for the client's team.
    pub fn eye(&self) -> Eye {
        pitch_camera(self.client.number("$Match::Attack"))
    }

    /// Replaces the client's socket (a browser's client opens a new game socket to join again).
    pub fn set_socket(&mut self, socket: Box<dyn Datagrams>) {
        let st = self.client.host.client.borrow();
        let mut n = st.net.borrow_mut();
        n.socket = Some(socket);
        n.policy = NetPolicy::Loopback;
    }

    /// Shows `text` on the main menu's status line, handed to `setText` as an argument (never built into script source).
    pub fn status(&mut self, text: &str) {
        let vm = self.client.vm_mut();
        for name in [&b"MainStatus"[..], b"HostStatus"] {
            if let Some(line) = vm.objects().find(name) {
                let _ = vm.call_method(line, b"setText", &[text.as_bytes().to_vec()]);
            }
        }
    }

    /// `ms` of the clock: the client's step (and the hosted server's ticks), then what the scripts asked of the host.
    pub fn step(&mut self, ms: u32) {
        self.client.step(ms);
        #[cfg(not(target_arch = "wasm32"))]
        if self.client.take_flag("$Game::HostAsked")
            && let Err(e) = self.start_match()
        {
            self.status(&format!("Could not start the match: {e}"));
        }
        if self.client.take_flag("$Game::LeaveAsked") {
            self.client.stop_hosting();
        }
    }

    /// Starts a match here as `$Game::HostScope` says (`public`: every interface; else the LAN) and joins it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_match(&mut self) -> Result<(), String> {
        use torque::app::hosting::{join_address, lan_bind_address};

        use crate::server::{ServerOptions, server_engine, start};

        self.client.stop_hosting();
        let lua = self
            .options
            .lua
            .as_ref()
            .ok_or("hosting needs the Lua extension (build it with `just lua`)")?;
        let engine = match self.server_engine.take() {
            Some(engine) => engine,
            None => server_engine(lua)?,
        };
        let engine = self.server_engine.insert(engine);
        let public = self.client.global("$Game::HostScope") == "public";
        let address = if public && self.options.net == NetMode::Lan {
            Ipv4Addr::UNSPECIFIED
        } else {
            lan_bind_address(self.options.net, self.options.host_address)
        };
        let options = ServerOptions {
            address,
            // A harness never takes a fixed port.
            port: self.options.net.host_port(self.options.port),
            name: format!("{}'s match", self.client.global("$Pref::Player::Name")),
            public,
            dedicated: false,
            ..ServerOptions::default()
        };
        let server = start(engine, &options, options.udp())?;
        let bound = server.address().ok_or("the server has no socket")?;
        self.client.start_hosting(server);
        self.join(&join_address(bound).to_string())
    }
}

impl App for SoccerGame {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "Soccer".to_string()
    }

    fn log_name(&self) -> &str {
        "mods-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        world_view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.dt = ms as f32 / 1000.0;
        self.step(ms);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let eye = self.eye();
        (
            self.drawing.frame(&mut self.client, view, self.dt, &eye),
            eye,
        )
    }
}
