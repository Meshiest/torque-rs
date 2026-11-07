//! A game's client: a client host (script engine, GUI, action maps, TGE's connection half and its server query) and, while it
//! hosts one, the server it runs in the same process on the same clock.
//!
//! **Made** by a [`ClientBuilder`]: the engine's natives registered as [`ClientSetup`] asks, then the game's own natives and
//! globals, its script run (`client.cs`) and its first screen shown; [`ClientBuilder::build`] makes the host.
//!
//! **The socket** ([`ClientNet`]): none (a client with no connection), a UDP socket, or an in-memory one linked to a server in
//! the same process ([`ClientNet::Memory`]): every step carries the datagrams each sent to the other, so the connection is the
//! engine's own handshake, packets and ghosts, with no socket opened. A client with a UDP socket can still run a game alone
//! that way ([`GameClient::start_local`]): its socket is set aside while the game runs, so nothing of it reaches the network.
//! A game may also bring its own socket ([`ClientNet::External`], [`ClientBuilder::set_socket`]): a browser's game socket
//! over WebSocket.
//!
//! **A step** ([`GameClient::step`]): as many server ticks as the time covers when hosting, then the client's ticks (its input,
//! its scripts, its connection, its query), the link carried around each, and the console's lines kept.

use std::any::Any;
use std::cell::RefCell;
use std::net::{SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque_client::conn::tge::{TgeConn, TgeWorld};
use torque_client::game::ClientGame;
use torque_client::host::{ClientHost, ClientPass};
use torque_client::hud::HudWorld as _;
use torque_client::input::{Key, key_message};
use torque_client::net::{Datagrams, NetPolicy};
use torque_client::state::ClientState;
use torque_engine::{ClientStage, ClientTick, Engine, EngineBuilder, PluginGroup};
use torque_gui::Gui;
use torque_server::server::{GameServer as _, Server, TICK_MS};
use torque_ts_text::{atof, atoi};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use super::Host;
use super::plugins::{
    ClientGamePlugin, ClientHostSetup, ClientWiring, DefaultPlugins, GameClassesPlugin, GuiPlugin,
    ServerQueryPlugin,
};
use super::server::GameServer;
use super::transports::{
    Connect, LinkInfo, MemoryLink, MemoryTarget, UdpTarget, client_transport_warnings, open_connect,
};

/// The most console lines a client keeps of its own and of its server's (it drops the older half past this).
pub const LOG_LINES: usize = 4000;

/// Where a client's socket binds and what it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientNet {
    /// No socket and no connection natives: a client that only draws its canvas.
    None,
    /// A UDP socket on `127.0.0.1` that reads only loopback datagrams (tests, a game for one).
    Loopback,
    /// A UDP socket bound as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1` hears its LAN
    /// query), under the engine's internet policy.
    Lan,
    /// An in-memory socket at `client`, linked to a server in this process listening at `server`.
    Memory {
        client: SocketAddrV4,
        server: SocketAddr,
    },
    /// The connection natives with no socket opened here: the game gives its own ([`ClientBuilder::set_socket`], a game
    /// socket over WebSocket in a browser).
    External,
}

/// What a client's script engine is given before the game's own natives.
pub struct ClientSetup {
    /// The files the scripts read (none: the scripts are evaluated from text).
    pub files: Option<MemFiles>,
    /// TGE's game classes (the datablocks a connection receives).
    pub game_classes: bool,
    /// The game's half of the client's prediction.
    pub client_game: Option<Rc<dyn ClientGame>>,
    /// Whether the GUI's layout measures text with the fonts it is drawn with ([`layout_fonts`](crate::layout_fonts)); else with its default.
    pub layout_fonts: bool,
    pub net: ClientNet,
    /// The LAN query's natives (the list of games).
    pub server_query: bool,
}

impl Default for ClientSetup {
    /// A client with no files, no game classes and no connection.
    fn default() -> ClientSetup {
        ClientSetup {
            files: None,
            game_classes: false,
            client_game: None,
            layout_fonts: false,
            net: ClientNet::None,
            server_query: false,
        }
    }
}

/// A client being made: its script engine, GUI and state, the engine's natives registered.
pub struct ClientBuilder {
    pub vm: Vm,
    pub gui: Gui,
    pub state: Rc<RefCell<ClientState<TgeConn>>>,
    link: Option<MemoryLink>,
    engine: Engine,
    pass: Rc<dyn ClientPass<TgeConn>>,
    host: ClientHostSetup,
}

impl ClientBuilder {
    /// The script engine with the GUI's classes, ActionMap's natives (the class itself is a plain SimObject, as TGE's) and the
    /// screen's, then what `setup` asks for. It is [`ClientBuilder::from_engine`] over [`DefaultPlugins`] with the plugins
    /// `setup` does not ask for turned off.
    pub fn new(setup: ClientSetup) -> Result<ClientBuilder, String> {
        let ClientSetup {
            files,
            game_classes,
            client_game,
            layout_fonts,
            net,
            server_query,
        } = setup;
        let mut group = DefaultPlugins.set(GuiPlugin { layout_fonts });
        if !game_classes {
            group = group.disable::<GameClassesPlugin>();
        }
        if !server_query {
            group = group.disable::<ServerQueryPlugin>();
        }
        let mut engine = EngineBuilder::new();
        engine.add_plugins(group);
        if let Some(game) = client_game {
            engine.add_plugins(ClientGamePlugin::new(game));
        }
        let engine = engine.build().map_err(|e| e.to_string())?;
        ClientBuilder::from_engine(&engine, files, net)
    }

    /// A client made by `engine`'s plugins: the script engine's client recipes run stage by stage ([`ClientStage`]) up to
    /// the game's natives, in the order the engine's plugins were added, with `files` as the scripts' files and a socket as
    /// `net` says. The engine can make any number of clients. It needs the plugins that make the client's parts:
    /// [`GuiPlugin`] and [`ClientHostPlugin`](crate::plugins::ClientHostPlugin) (with [`DefaultPlugins`], all of them). What the build warned about
    /// ([`Engine::warnings`]) is printed to the client's console first.
    pub fn from_engine(
        engine: &Engine,
        files: Option<MemFiles>,
        net: ClientNet,
    ) -> Result<ClientBuilder, String> {
        let pass = engine
            .slot::<dyn ClientPass<TgeConn>>()
            .ok_or("the engine has no client host (ClientHostPlugin)")?;
        let host = *engine
            .resource::<ClientHostSetup>()
            .ok_or("the engine has no client host (ClientHostPlugin)")?;
        let mut vm = Vm::new();
        for warning in engine.warnings() {
            vm.warn(warning.to_string());
        }
        for warning in client_transport_warnings(engine) {
            vm.warn(warning);
        }
        let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        let link = open_socket(engine, &state, net)?;
        let wiring = Rc::new(ClientWiring::new(Rc::clone(&state), net));
        vm.set_extension(Rc::clone(&wiring));
        replay(engine, ClientStage::Classes, &mut vm)?;
        if let Some(files) = files {
            vm.set_file_host(files);
        }
        if let Some(game) = engine.slot::<dyn ClientGame>() {
            torque_client::game::set_client_game(&mut vm, game);
        }
        for stage in [
            ClientStage::Game,
            ClientStage::Gui,
            ClientStage::Input,
            ClientStage::Screen,
            ClientStage::Conn,
            ClientStage::Query,
            ClientStage::Natives,
        ] {
            replay(engine, stage, &mut vm)?;
        }
        let gui = wiring.gui().ok_or("the engine has no GUI (GuiPlugin)")?;
        Ok(ClientBuilder {
            vm,
            gui,
            state,
            link,
            engine: engine.clone(),
            pass,
            host,
        })
    }

    /// Gives the client `socket` under `policy` (a client made with [`ClientNet::External`]; any other socket it had is
    /// closed).
    pub fn set_socket(&mut self, socket: Box<dyn Datagrams>, policy: NetPolicy) {
        let st = self.state.borrow();
        let mut n = st.net.borrow_mut();
        n.policy = policy;
        n.socket = Some(socket);
    }

    /// Connects the client over the transport registered under `id`, to `target` (of the type that transport takes; see
    /// [`transports`](crate::transports)): a client made with [`ClientNet::External`], or one that changes its transport. Any
    /// other socket it had is closed. Gives back what the transport has to say about its link ([`LinkInfo`]). Err: no
    /// transport is registered under `id` (its plugin is off), the target is not its type, or it could not open.
    pub fn connect<T: Any>(&mut self, id: &str, target: &T) -> Result<LinkInfo, String> {
        self.connect_dyn(id, target)
    }

    /// Runs the script file `path` of the files.
    pub fn exec(&mut self, path: &str) -> Result<(), String> {
        if self
            .vm
            .exec_file(path.as_bytes(), false)
            .map_err(|e| format!("{path}: {e}"))?
        {
            Ok(())
        } else {
            Err(format!("{path} is not in the game folder"))
        }
    }

    /// Calls the script function `function` with no arguments.
    pub fn invoke(&mut self, function: &str) -> Result<(), String> {
        self.vm
            .invoke(None, function.as_bytes(), &[])
            .map(drop)
            .map_err(|e| format!("{function}: {e}"))
    }

    /// The client host over the engine, in a window's mode, with no server hosted yet.
    pub fn build<S>(mut self) -> GameClient<S> {
        // The host takes the script engine, so the `Host` stage's recipes are the last to see it.
        if let Err(e) = self.engine.replay(ClientStage::Host, &mut self.vm) {
            self.vm.warn(e.to_string());
        }
        let ticks = self.engine.list::<ClientTick>();
        let pass = if ticks.is_empty() {
            self.pass
        } else {
            Rc::new(TickedPass {
                pass: self.pass,
                ticks: ticks.to_vec(),
            })
        };
        let mut host = ClientHost::new(
            Server::new(self.vm),
            self.gui,
            self.state,
            pass,
            (self.host.draw_cache)(),
        );
        if self.host.windowed {
            host.set_windowed();
        }
        let mut client = GameClient {
            host,
            engine: self.engine,
            server: None,
            link: self.link,
            parked: None,
            server_lag: 0,
            frame_dt: 0.0,
            server_log: Vec::new(),
            client_log: Vec::new(),
        };
        client.set_global("$Client::HostsServer", "0");
        client
    }
}

impl Connect for ClientBuilder {
    fn connect_dyn(&mut self, id: &str, target: &dyn Any) -> Result<LinkInfo, String> {
        let (socket, policy, link, info) = open_connect(&self.engine, id, target)?.split();
        self.set_socket(socket, policy);
        self.link = link;
        Ok(info)
    }
}

/// The engine's pass with the plugins' per-tick work ([`ClientTick`]) after it, in the order the plugins pushed it.
struct TickedPass {
    pass: Rc<dyn ClientPass<TgeConn>>,
    ticks: Vec<ClientTick>,
}

impl ClientPass<TgeConn> for TickedPass {
    fn net(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
        self.pass.net(vm, client);
    }

    fn other(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
        self.pass.other(vm, client);
        for tick in &self.ticks {
            tick.run(vm);
        }
    }
}

/// Replays `stage` of `engine`'s client recipes on `vm`.
fn replay(engine: &Engine, stage: ClientStage, vm: &mut Vm) -> Result<(), String> {
    engine.replay(stage, vm).map_err(|e| e.to_string())
}

/// Opens the client's socket as `net` says, over the engine's transports (`udp` for [`ClientNet::Loopback`] and
/// [`ClientNet::Lan`], `memory` for [`ClientNet::Memory`]); an in-memory one comes with its link.
fn open_socket(
    engine: &Engine,
    state: &Rc<RefCell<ClientState<TgeConn>>>,
    net: ClientNet,
) -> Result<Option<MemoryLink>, String> {
    let (id, target): (&str, &dyn Any) = match &net {
        ClientNet::None | ClientNet::External => return Ok(None),
        ClientNet::Loopback => ("udp", &UdpTarget::Loopback),
        ClientNet::Lan => ("udp", &UdpTarget::Lan),
        ClientNet::Memory { client, server } => (
            "memory",
            &MemoryTarget {
                client: *client,
                server: *server,
            },
        ),
    };
    let (socket, policy, link, _) = open_connect(engine, id, target)?.split();
    let st = state.borrow();
    let mut n = st.net.borrow_mut();
    n.policy = policy;
    n.socket = Some(socket);
    Ok(link)
}

/// What [`GameClient::start_local`] set aside.
struct Parked {
    socket: Option<Box<dyn Datagrams>>,
    policy: NetPolicy,
    link: Option<MemoryLink>,
}

/// A game's client, and the server `S` it hosts: a [`GameServer`], or a game's own type over one (borrowed as one, and made
/// one when it stops).
pub struct GameClient<S = GameServer> {
    pub host: Host,
    engine: Engine,
    /// The server this client runs in its own process, if any.
    pub server: Option<S>,
    link: Option<MemoryLink>,
    /// The socket, its policy and the link a game played alone set aside ([`GameClient::start_local`]), given back when it stops.
    parked: Option<Parked>,
    /// Milliseconds of the client's clock the hosted server has not ticked yet.
    server_lag: u32,
    /// The seconds the next frame covers.
    frame_dt: f32,
    /// Lines the hosted server printed (kept for the tests and the console).
    pub server_log: Vec<String>,
    /// Lines the client printed.
    pub client_log: Vec<String>,
}

impl<S> Connect for GameClient<S> {
    fn connect_dyn(&mut self, id: &str, target: &dyn Any) -> Result<LinkInfo, String> {
        let (socket, policy, link, info) = open_connect(&self.engine, id, target)?.split();
        {
            let st = self.host.client.borrow();
            let mut n = st.net.borrow_mut();
            n.policy = policy;
            n.socket = Some(socket);
        }
        self.link = link;
        Ok(info)
    }
}

impl<S> GameClient<S> {
    /// Connects the client over the transport registered under `id`, to `target` (of the type that transport takes; see
    /// [`transports`](crate::transports)), closing any other socket it had: a browser's client opens a new link to join
    /// again. Not while a game is played alone ([`GameClient::start_local`]). Err: no transport is registered under `id`, the
    /// target is not its type, or it could not open.
    pub fn connect<T: Any>(&mut self, id: &str, target: &T) -> Result<LinkInfo, String> {
        self.connect_dyn(id, target)
    }
}

impl<S> GameClient<S> {
    pub fn vm(&self) -> &Vm {
        self.host.server.vm()
    }

    pub fn vm_mut(&mut self) -> &mut Vm {
        self.host.server.vm_mut()
    }

    /// A key going down or up, as the window posts it.
    pub fn key(&mut self, key: Key, down: bool) {
        let (msg, wparam, lparam) = key_message(key, down, false, false);
        self.host.post(msg, wparam, lparam);
    }

    /// Calls the script function `function` with `args` in the client's engine (tests and pictures press a menu's buttons and
    /// keys this way) and returns what it returned.
    pub fn call(&mut self, function: &str, args: &[&str]) -> Result<String, String> {
        let argv: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        self.vm_mut()
            .invoke(None, function.as_bytes(), &argv)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{function}: {e}"))
    }

    /// Where the control `name` is on the screen: its left, top, width and height (the script's `getScreenPosition` and
    /// `getExtent`).
    pub fn control_rect(&mut self, name: &str) -> Result<[i32; 4], String> {
        let code = format!("return {name}.getScreenPosition() SPC {name}.getExtent();");
        let text = self.call("eval", &[&code])?;
        let n: Vec<i32> = text
            .split_whitespace()
            .filter_map(|w| w.parse().ok())
            .collect();
        <[i32; 4]>::try_from(n).map_err(|_| format!("{name}: no place on the screen ({text:?})"))
    }

    /// Sets a global of the client's scripts.
    pub fn set_global(&mut self, name: &str, value: &str) {
        self.vm_mut().set_global(name, value);
    }

    /// A global of the client's scripts (empty when unset).
    pub fn global(&self, name: &str) -> String {
        self.vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    /// Whether a global is set to a number other than 0.
    pub fn flag(&self, name: &str) -> bool {
        self.vm().global(name).is_some_and(|v| atoi(&v) != 0)
    }

    /// A global as a number (0 when unset).
    pub fn number(&self, name: &str) -> f32 {
        self.vm().global(name).map_or(0.0, |v| atof(&v) as f32)
    }

    /// A flag the scripts set for the host, taken (cleared when set).
    pub fn take_flag(&mut self, name: &str) -> bool {
        let vm = self.vm_mut();
        let set = vm.global(name).is_some_and(|v| atoi(&v) != 0);
        if set {
            vm.set_global(name, "");
        }
        set
    }

    /// A number the scripts set for the host, taken (cleared); None when unset or empty.
    pub fn take_number(&mut self, name: &str) -> Option<i32> {
        let vm = self.vm_mut();
        let v = vm.global(name).filter(|v| !v.is_empty())?;
        vm.set_global(name, "");
        Some(atoi(&v))
    }

    /// The client's world of ghosts, if it is connected.
    pub fn with_world<R>(&self, f: impl FnOnce(&TgeWorld) -> R) -> Option<R> {
        let st = self.host.client.borrow();
        let conn = st.conn.conn.as_ref()?;
        Some(f(&conn.world))
    }

    /// How many ghosts the client holds.
    pub fn ghost_count(&self) -> usize {
        self.with_world(|w| w.records.ghosts.len()).unwrap_or(0)
    }

    /// The map's Sky, once its ghost came: how far the view reaches and where the haze starts (`visibleDistance` and
    /// `fogDistance`, capped by the frame's view distance).
    pub fn sky_distances(&self) -> Option<(f32, f32)> {
        self.with_world(|w| {
            w.records
                .ghosts
                .values()
                .any(|g| g.class == Some(torque_ghosts::TgeClass::Sky))
                .then(|| w.view_distances())
        })
        .flatten()
    }

    /// Adds `seconds` to the time the next frame covers.
    pub fn add_frame_time(&mut self, seconds: f32) {
        self.frame_dt += seconds;
    }

    /// The time the next frame covers (seconds), taken.
    pub fn take_frame_time(&mut self) -> f32 {
        std::mem::take(&mut self.frame_dt)
    }

    /// The control object's view this frame: the predicted Player between its last two ticks (`back`: 1 right after a tick, 0
    /// just before the next), its angles drawn ahead by the move globals the next move will be made of.
    pub fn predict_frame(&mut self) {
        let host = &self.host;
        let vm = host.server.vm();
        let now = vm.sim_time();
        let mut st = host.client.borrow_mut();
        let Some(c) = st.conn.conn.as_mut() else {
            return;
        };
        let c = &mut **c;
        let Some(last) = c.last_tick else {
            return;
        };
        let since = now.wrapping_sub(last).min(TICK_MS);
        c.predict.back = 1.0 - since as f32 / TICK_MS as f32;
        c.world.anim_ms = now;
        c.predict.set_aim_input(vm);
        // The wheels of the vehicles the client runs spin by the frame's time (`WheeledVehicle::advanceTime`).
        c.predict.frame_vehicles(now);
        c.predict.update_view(&mut c.world);
    }
}

impl<S: std::borrow::BorrowMut<GameServer> + Into<GameServer>> GameClient<S> {
    /// `ms` of the clock (see the module docs).
    pub fn step(&mut self, ms: u32) {
        let mut left = ms;
        while left > 0 {
            let slice = left.min(TICK_MS);
            left -= slice;
            if let Some(server) = self.server.as_mut() {
                let server: &mut GameServer = std::borrow::BorrowMut::borrow_mut(server);
                self.server_lag += slice;
                while self.server_lag >= TICK_MS {
                    self.server_lag -= TICK_MS;
                    if let Some(link) = &self.link {
                        link.carry(server);
                    }
                    server.tick();
                    if let Some(link) = &self.link {
                        link.carry(server);
                    }
                }
                self.server_log.extend(server.drain_log());
            }
            self.host.run(slice, |_| false);
            if let (Some(server), Some(link)) = (self.server.as_ref(), &self.link) {
                link.carry(std::borrow::Borrow::borrow(server));
            }
            let lines = self.host.drain_log();
            self.client_log.extend(
                lines
                    .into_iter()
                    .map(|l| String::from_utf8_lossy(&l.text).into_owned()),
            );
        }
        for log in [&mut self.server_log, &mut self.client_log] {
            if log.len() > LOG_LINES {
                let cut = log.len() - LOG_LINES / 2;
                log.drain(..cut);
            }
        }
    }

    /// Runs `server` here from now on (its lines so far kept): the client then joins it at its address. The scripts see
    /// `$Client::HostsServer` 1 until hosting stops.
    pub fn start_hosting(&mut self, mut server: S) {
        self.server_log
            .extend(std::borrow::BorrowMut::<GameServer>::borrow_mut(&mut server).drain_log());
        self.server = Some(server);
        self.server_lag = 0;
        self.set_global("$Client::HostsServer", "1");
    }

    /// Runs `server`, started on [`super::server::Listen::Memory`] at `at`, here over an in-memory link from `client` (a game
    /// played alone): the client's socket is set aside until hosting stops, so nothing goes out on the network. The client then
    /// joins it at `at`. Err: the engine has no `memory` transport (its plugin is off).
    pub fn start_local(
        &mut self,
        server: S,
        client: SocketAddrV4,
        at: SocketAddr,
    ) -> Result<(), String> {
        self.stop_hosting();
        let target = MemoryTarget { client, server: at };
        let (socket, policy, link, _) = open_connect(&self.engine, "memory", &target)?.split();
        {
            let st = self.host.client.borrow();
            let mut n = st.net.borrow_mut();
            let socket = n.socket.replace(socket);
            let policy = std::mem::replace(&mut n.policy, policy);
            self.parked = Some(Parked {
                socket,
                policy,
                link: std::mem::replace(&mut self.link, link),
            });
        }
        self.start_hosting(server);
        Ok(())
    }

    /// Whether the client plays a game alone ([`GameClient::start_local`]).
    pub fn local(&self) -> bool {
        self.parked.is_some()
    }

    /// Stops the hosted server, if there is one: its socket closed, its scripts' end run, the link's datagrams dropped, and the
    /// socket a game played alone set aside given back.
    pub fn stop_hosting(&mut self) {
        if let Some(server) = self.server.take() {
            let mut server: GameServer = server.into();
            server.close();
            let _ = server.server.finish();
        }
        if let Some(link) = &self.link {
            link.clear();
        }
        if let Some(p) = self.parked.take() {
            let st = self.host.client.borrow();
            let mut n = st.net.borrow_mut();
            n.socket = p.socket;
            n.policy = p.policy;
            self.link = p.link;
        }
        self.set_global("$Client::HostsServer", "0");
    }

    /// The hosted server's address, if this client hosts one.
    pub fn hosted_address(&self) -> Option<SocketAddr> {
        self.server
            .as_ref()
            .and_then(|s| std::borrow::Borrow::<GameServer>::borrow(s).address())
    }
}
