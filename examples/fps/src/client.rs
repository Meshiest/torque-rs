//! The example's client, and the server it hosts: a client host (script engine, GUI, action maps, TGE's connection half and its
//! server query) on its own UDP socket, whose canvas is the menu, the list of games or the arena's view, and while it hosts, an
//! [`ArenaServer`] on the same clock.
//!
//! **The socket.** [`NetMode::Loopback`] binds the client's socket to `127.0.0.1` and reads only loopback datagrams (the tests,
//! a game for one); [`NetMode::Lan`] binds it as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1`
//! hears its LAN query) and talks to LAN addresses only.
//!
//! **A step** ([`ArenaGame::step`]): as many server ticks as the time covers when hosting, then the client's ticks (its input,
//! its scripts, its connection, its query); then what the scripts asked of the host: a game hosted (`$Arena::HostAsked`: the
//! server starts and the client joins it over the loopback address) or the arena left (`$Arena::LeaveAsked`: the server stops).
//!
//! **A frame** ([`ArenaGame::frame`]): the client's own figure where the client predicts it, between its last two ticks
//! (`Player::interpolateTick`) and turned ahead by the mouse's waiting turn ([`crate::prediction`]), the other figures where the
//! client runs them between their records, the ghosts as the world view's objects ([`crate::scene`]), the canvas drawn, and
//! the eye at the own figure's eye.
//!
//! Credit: the frame between the last two ticks follows `Player::interpolateTick` and the client's `ProcessList` in Torque3D's
//! MIT-licensed `T3D/player.cpp` and `T3D/gameBase/processList.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015
//! Faust Logic, Inc. for `player.cpp`). The code here is this example's own.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque::client::conn::tge::{TgeConn, TgeWorld};
use torque::client::host::{ClientHost, EnginePass};
use torque::client::hud::HudWorld as _;
use torque::client::input::{WM_KEYDOWN, WM_KEYUP, key_lparam};
use torque::client::net::{NetPolicy, UdpDatagrams};
use torque::client::state::ClientState;
use torque::gui::GuiDraws;
use torque::script::text::atoi;
use torque::script::vm::vm::Vm;
use torque::server::server::{GameServer as _, Server, TICK_MS};
use torque_example_common::draw_cache;
use torque_example_common::view::{Daylight, WorldView};
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::files::GameFiles;
use crate::prediction::ArenaClient;
use crate::scene::{ArenaScene, OwnLook, TextureFilter};
use crate::server::{ArenaServer, DEFAULT_PORT, ServerOptions};
use torque::client::conn::tge::TgeWorld as World;
use torque::ghost_world::world::Pose;

/// Where the client's socket binds and what it reaches (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetMode {
    Loopback,
    Lan,
}

/// How a client is made.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub net: NetMode,
    /// The player's name (the menu's field starts with it).
    pub name: String,
    /// The UDP port number the LAN list asks and a hosted game binds (0 when hosting: any free one).
    pub port: u16,
    /// The address a hosted game binds.
    pub host_address: Ipv4Addr,
    /// The map a hosted game plays (a path in the game folder); none: the first in `missions/`.
    pub map: Option<String>,
}

impl Default for ClientOptions {
    fn default() -> ClientOptions {
        ClientOptions {
            net: NetMode::Lan,
            name: "Player".to_string(),
            port: DEFAULT_PORT,
            host_address: Ipv4Addr::LOCALHOST,
            map: None,
        }
    }
}

/// The fonts the GUI's layout measures text with: the same faces [`draw_cache`] draws with.
fn layout_fonts() -> torque::fonts::FontCache {
    let fallback: Box<dyn torque::fonts::Fallback> = match torque::fonts::system::SystemFonts::new()
    {
        Some(system) => Box::new(system),
        None => Box::new(torque::fonts::NoFallback),
    };
    torque::fonts::FontCache::new("fonts", fallback)
}

/// The virtual keys the example uses.
pub mod keys {
    pub const W: u8 = 0x57;
    pub const A: u8 = 0x41;
    pub const S: u8 = 0x53;
    pub const D: u8 = 0x44;
    pub const T: u8 = 0x54;
    pub const SPACE: u8 = 0x20;
    pub const ESCAPE: u8 = 0x1b;
    pub const RETURN: u8 = 0x0d;
}

/// The example's client, and the server it hosts.
pub struct ArenaGame {
    pub host: ClientHost<TgeConn, Server>,
    pub files: GameFiles,
    pub options: ClientOptions,
    pub server: Option<ArenaServer>,
    scene: ArenaScene,
    /// Milliseconds of the client's clock the hosted server has not ticked yet.
    server_lag: u32,
    /// The seconds the last frame covered.
    frame_dt: f32,
    /// The Sky's distances the view was given.
    sky: Option<(f32, f32)>,
    /// Lines the hosted server printed (kept for the tests and the console).
    pub server_log: Vec<String>,
    /// Lines the client printed.
    pub client_log: Vec<String>,
}

impl ArenaGame {
    /// The client over `files`: `client.cs` run, the main menu showing.
    pub fn new(files: GameFiles, options: ClientOptions) -> Result<ArenaGame, String> {
        let mut vm = Vm::new();
        torque::game::specs::install(&mut vm);
        vm.set_file_host(files.mem_files());
        torque::client::game::set_client_game(
            &mut vm,
            Rc::new(ArenaClient::new(files.mem_files())),
        );
        let gui = torque::gui::register(&mut vm);
        // The controls that measure text as they lay out (the list of games) find their fonts where the drawing does: the
        // system's TrueType faces.
        gui.0.borrow_mut().fonts = torque::gui::SharedFonts::new(Box::new(layout_fonts()));
        let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        {
            let (ip, policy, twin) = match options.net {
                NetMode::Loopback => (Ipv4Addr::LOCALHOST, NetPolicy::Loopback, false),
                NetMode::Lan => (Ipv4Addr::UNSPECIFIED, NetPolicy::Lan, true),
            };
            let socket =
                UdpDatagrams::open(ip, 0, twin).map_err(|e| format!("the client's socket: {e}"))?;
            let st = state.borrow();
            let mut net = st.net.borrow_mut();
            net.policy = policy;
            net.socket = Some(Box::new(socket));
        }
        // The engine's natives on ActionMap; the class itself is the game's to declare (TGE's is a plain SimObject).
        vm.register_class("ActionMap", "SimObject", &[]);
        torque::client::actionmap::register(&mut vm, &state);
        torque::client::screen::register(&mut vm, &gui, &state);
        torque::client::conn::register(&mut vm, &state, &gui);
        torque::client::query::register(&mut vm, &state);
        vm.set_global("$Arena::Port", &options.port.to_string());
        if !vm
            .exec_file(b"client.cs", false)
            .map_err(|e| format!("client.cs: {e}"))?
        {
            return Err("client.cs is not in the game folder".to_string());
        }
        vm.set_global("$Pref::Player::Name", &options.name);
        vm.invoke(None, b"showMainMenu", &[])
            .map_err(|e| format!("showMainMenu: {e}"))?;
        let mut host = ClientHost::new(
            Server::new(vm),
            gui,
            state,
            Rc::new(EnginePass),
            draw_cache(),
        );
        host.set_windowed();
        Ok(ArenaGame {
            host,
            files,
            options,
            server: None,
            scene: ArenaScene::default(),
            server_lag: 0,
            frame_dt: 0.0,
            sky: None,
            server_log: Vec::new(),
            client_log: Vec::new(),
        })
    }

    pub fn vm(&self) -> &Vm {
        self.host.server.vm()
    }

    pub fn vm_mut(&mut self) -> &mut Vm {
        self.host.server.vm_mut()
    }

    /// A world view on `gpu`.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    /// A key going down or up, as the window posts it.
    pub fn key(&mut self, vk: u8, down: bool) {
        let msg = if down { WM_KEYDOWN } else { WM_KEYUP };
        self.host
            .post(msg, u32::from(vk), key_lparam(vk, !down, false));
    }

    /// Calls the script function `function` with `args` in the client's engine (the tests and the pictures press the menu's
    /// buttons and keys this way) and returns what it returned.
    pub fn call(&mut self, function: &str, args: &[&str]) -> Result<String, String> {
        let argv: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        self.vm_mut()
            .invoke(None, function.as_bytes(), &argv)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{function}: {e}"))
    }

    /// Sets a global of the client's scripts.
    pub fn set_global(&mut self, name: &str, value: &str) {
        self.vm_mut().set_global(name, value);
    }

    /// A global of the client's scripts.
    pub fn global(&self, name: &str) -> String {
        self.vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    /// Starts a game here: its server, and the client joins it.
    pub fn host_game(&mut self) -> Result<(), String> {
        self.stop_hosting();
        let mission = match self.options.map.clone() {
            Some(map) => map,
            None => self
                .files
                .maps()
                .into_iter()
                .next()
                .ok_or("no map in the game folder's missions")?,
        };
        let name = format!("{}'s game", self.global("$Pref::Player::Name"));
        let port = if self.options.net == NetMode::Loopback {
            0
        } else {
            self.options.port
        };
        let mut server = ArenaServer::start(
            &self.files,
            &ServerOptions {
                address: self.options.host_address,
                port,
                name,
                mission,
            },
        )?;
        self.server_log.extend(server.drain_log());
        let address = server.address().ok_or("the server has no socket")?;
        let join = SocketAddr::new(address.ip(), address.port()).to_string();
        self.server = Some(server);
        self.server_lag = 0;
        self.vm_mut()
            .invoke(None, b"connectTo", &[join.into_bytes()])
            .map_err(|e| format!("connectTo: {e}"))?;
        Ok(())
    }

    /// The hosted server's address, if this client hosts one.
    pub fn hosted_address(&self) -> Option<SocketAddr> {
        self.server.as_ref().and_then(ArenaServer::address)
    }

    /// Stops the hosted server, if there is one.
    pub fn stop_hosting(&mut self) {
        if let Some(mut server) = self.server.take() {
            server.close();
            let _ = server.server.finish();
        }
        self.scene.clear();
    }

    /// A global the scripts set for the host, taken (cleared).
    fn take_flag(&mut self, name: &str) -> bool {
        let vm = self.vm_mut();
        let set = vm.global(name).is_some_and(|v| atoi(&v) != 0);
        if set {
            vm.set_global(name, "");
        }
        set
    }

    /// `ms` of the clock (see the module docs).
    pub fn step(&mut self, ms: u32) {
        let mut left = ms;
        while left > 0 {
            let slice = left.min(TICK_MS);
            left -= slice;
            if let Some(server) = self.server.as_mut() {
                self.server_lag += slice;
                while self.server_lag >= TICK_MS {
                    self.server_lag -= TICK_MS;
                    server.tick();
                }
                self.server_log.extend(server.drain_log());
            }
            self.host.run(slice, |_| false);
            let lines = self.host.drain_log();
            self.client_log.extend(
                lines
                    .into_iter()
                    .map(|l| String::from_utf8_lossy(&l.text).into_owned()),
            );
        }
        for log in [&mut self.server_log, &mut self.client_log] {
            if log.len() > 4000 {
                let cut = log.len() - 2000;
                log.drain(..cut);
            }
        }
        if self.take_flag("$Arena::HostAsked")
            && let Err(e) = self.host_game()
        {
            eprintln!("example-fps: {e}");
            let text = format!("Could not start the game: {e}");
            if let Some(status) = self.vm().objects().find(b"MainStatus") {
                let _ = self
                    .vm_mut()
                    .call_method(status, b"setText", &[text.into_bytes()]);
            }
        }
        if self.take_flag("$Arena::LeaveAsked") {
            self.stop_hosting();
        }
    }

    /// Whether the canvas shows the arena (the server said the game started).
    pub fn in_game(&self) -> bool {
        atoi(self.global("$Arena::InGame").as_bytes()) != 0
    }

    /// Whether the scripts asked to quit.
    pub fn quit_asked(&self) -> bool {
        atoi(self.global("$Arena::QuitAsked").as_bytes()) != 0
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

    /// Adds `seconds` to the time the next frame covers.
    pub fn add_frame_time(&mut self, seconds: f32) {
        self.frame_dt += seconds;
    }

    /// The scene as the last frame drew it.
    pub fn scene(&self) -> &ArenaScene {
        &self.scene
    }

    /// The eye this frame: the own figure's eye, else a fixed view over the arena.
    pub fn eye(&self) -> Eye {
        self.scene.eye()
    }

    /// The map's Sky, once its ghost came: how far the view reaches and where the haze starts (`visibleDistance`,
    /// `fogDistance`); before it, the world view's own.
    fn apply_sky(&mut self, view: &mut WorldView) {
        let sky = self.with_world(|w| {
            w.records
                .ghosts
                .values()
                .any(|g| g.class == Some(torque::ghosts::TgeClass::Sky))
                .then(|| w.view_distances())
        });
        let Some(Some((visible, fog))) = sky else {
            return;
        };
        if self.sky == Some((visible, fog)) {
            return;
        }
        self.sky = Some((visible, fog));
        view.set_daylight(&Daylight {
            visible,
            fog_start: fog.min(visible),
            ..Daylight::default()
        });
    }

    /// The own figure's ghost (`$Arena::OwnGhost`, which the server names) and where the client predicts it: its feet, yaw and
    /// head as this frame draws them (the control object's, [`ArenaGame::view_frame`]). Before the prediction starts, the
    /// view's turn as the scripts keep it (`$Arena::LookYaw`, `$Arena::LookPitch`) and the record's feet.
    pub fn own_look(&self) -> Option<OwnLook> {
        let ghost = u32::try_from(atoi(self.global("$Arena::OwnGhost").as_bytes())).ok()?;
        let predicted = self
            .with_world(|w| match (w.control.object, w.control_render) {
                (
                    Some(index),
                    Some(Pose::Euler {
                        position,
                        pitch,
                        yaw,
                    }),
                ) if index == ghost => Some((position, yaw, pitch)),
                _ => None,
            })
            .flatten();
        if let Some((position, yaw, pitch)) = predicted {
            return Some(OwnLook {
                ghost,
                yaw,
                pitch,
                position: Some(position),
            });
        }
        let float = |name: &str| self.global(name).trim().parse::<f32>().unwrap_or(0.0);
        Some(OwnLook {
            ghost,
            yaw: float("$Arena::LookYaw"),
            pitch: float("$Arena::LookPitch"),
            position: None,
        })
    }

    /// Turns the own view to `pitch` (positive looks down) with the next move, as the mouse would turn it.
    pub fn set_view_pitch(&mut self, pitch: f32) {
        let now = self.own_look().map_or(0.0, |o| o.pitch);
        let waiting = self.vm().global_f32("$mvPitch").unwrap_or(0.0);
        self.set_global("$mvPitch", &(waiting + pitch - now).to_string());
        self.set_global("$Arena::LookPitch", &pitch.to_string());
    }

    /// The control object's view this frame: the predicted Player between its last two ticks (`back`: 1 right after a tick, 0
    /// just before the next), its angles drawn ahead by the move globals the next move will be made of.
    fn predict_frame(&mut self) {
        let now = self.host.server.vm().sim_time();
        let vm = self.host.server.vm();
        let mut st = self.host.client.borrow_mut();
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
        c.predict.update_view(&mut c.world);
    }

    /// The frame's motion, with nothing drawn: the own figure where the client predicts it and the other figures moved on by the
    /// time since the last frame. Returns that time (seconds).
    pub fn view_frame(&mut self) -> f32 {
        let dt = std::mem::take(&mut self.frame_dt);
        self.predict_frame();
        let own = self.own_look();
        let st = self.host.client.borrow();
        match st.conn.conn.as_ref() {
            Some(conn) => {
                let world: &World = &conn.world;
                let ghosts = &conn.predict.ghost_players;
                self.scene
                    .update_figures(&self.files.kit, world, ghosts, own, dt);
                self.scene.update_bolts(world, dt);
            }
            None => self.scene.clear(),
        }
        dt
    }

    /// The frame: the ghosts into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let dt = self.view_frame();
        let filter = TextureFilter::named(&self.global("$Arena::TextureFilter"));
        self.scene.set_texture_filter(filter);
        let objects = {
            let st = self.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.scene.objects(view, &self.files.kit, &conn.world, dt),
                None => Vec::new(),
            }
        };
        view.set_objects(objects);
        self.apply_sky(view);
        // The example draws the world itself: the GameTSCtrl draws the world whatever the connection's camera is.
        self.host.gui.0.borrow_mut().world_camera = true;
        (self.host.draw_frame(), self.eye())
    }
}
