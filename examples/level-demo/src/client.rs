//! The example's client and the server it runs: a client host (script engine, GUI, action maps, TGE's connection half) whose
//! canvas is the menu or the world's view, and while a map is played, the [`WorldServer`] it joined, both on one clock.
//!
//! **The link.** The client's socket is an in-memory one ([`MemoryDatagrams`]) and the server's transport another; every step
//! carries the datagrams each sent to the other, so the connection is the engine's own handshake, packets and ghosts, with no
//! socket opened.
//!
//! **A step** ([`LevelDemo::step`]): the client's ticks (its input, its scripts, its connection: the moves out, the ghosts in)
//! and as many server ticks as the time covers, the link carried around each; then what the scripts asked of the host: a map
//! picked (`$World::Pick`: the map's server starts and the client joins it) or the world left (`$World::Leave`: the server
//! stops).
//!
//! **A frame** ([`LevelDemo::frame`]): the ghosts as the world view's objects ([`crate::scene`]), the canvas drawn, and the eye:
//! behind the client's own figure (or at its eyes, `V`), kept in front of the cubes between.

use std::cell::RefCell;
use std::net::{SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque::client::conn::tge::{TgeConn, TgeWorld};
use torque::client::host::{ClientHost, EnginePass};
use torque::client::input::{WM_KEYDOWN, WM_KEYUP, key_lparam};
use torque::client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque::client::state::ClientState;
use torque::gui::GuiDraws;
use torque::script::text::atoi;
use torque::script::vm::host::MemFiles;
use torque::script::vm::vm::Vm;
use torque::server::server::{GameServer as _, Server, TICK_MS};
use torque_example_common::draw_cache;
use torque_example_common::view::WorldView;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::files::{GameFiles, MapEntry};
use crate::scene::{FigureView, WorldScene};
use crate::server::{SERVER_ADDR, WorldServer};

/// The client's address on the link.
pub const CLIENT_ADDR: SocketAddrV4 = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50_000);

/// The third-person camera: how far behind the figure, how high above its feet it looks, and how much it looks down.
pub const CAMERA_DISTANCE: f32 = 6.5;
pub const CAMERA_TARGET_HEIGHT: f32 = 1.7;
pub const CAMERA_PITCH: f32 = 0.3;
/// The first-person eye's height above the feet.
pub const EYE_HEIGHT: f32 = 1.75;
/// The nearest the camera comes to a cube between it and the figure.
pub const CAMERA_CLEARANCE: f32 = 0.3;

/// The virtual keys the example uses.
pub mod keys {
    pub const W: u8 = 0x57;
    pub const A: u8 = 0x41;
    pub const S: u8 = 0x53;
    pub const D: u8 = 0x44;
    pub const V: u8 = 0x56;
    pub const R: u8 = 0x52;
    pub const SHIFT: u8 = 0x10;
    pub const SPACE: u8 = 0x20;
    pub const ESCAPE: u8 = 0x1b;
    pub const ONE: u8 = 0x31;
}

/// The datagrams between the client's socket and the server's transport.
struct Link {
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Link {
    fn carry(&self, server: &WorldServer) {
        let client = SocketAddr::V4(CLIENT_ADDR);
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER_ADDR {
                server.end.inject(client, &data);
            }
        }
        let SocketAddr::V4(from) = SERVER_ADDR else {
            return;
        };
        let mut incoming = self.incoming.borrow_mut();
        for (to, data) in server.end.take_sent() {
            if to == client {
                incoming.push_back((from, data));
            }
        }
    }
}

/// The example's client, and the server of the map being played.
pub struct LevelDemo {
    pub host: ClientHost<TgeConn, Server>,
    pub files: GameFiles,
    pub maps: Vec<MapEntry>,
    pub server: Option<WorldServer>,
    link: Link,
    scene: WorldScene,
    /// Milliseconds of the client's clock the server has not ticked yet.
    server_lag: u32,
    /// The map being played (its index in [`LevelDemo::maps`]).
    pub playing: Option<usize>,
    /// The seconds the last frame covered (the figures move on by it).
    frame_dt: f32,
    /// Lines the server printed (kept for the tests and the console).
    pub server_log: Vec<String>,
}

impl LevelDemo {
    /// The client over `files`: `client.cs` run, the menu built from the maps found.
    pub fn new(files: GameFiles) -> Result<LevelDemo, String> {
        let maps = files.maps();
        let mut vm = Vm::new();
        torque::game::specs::install(&mut vm);
        let mem = MemFiles::new();
        files.fill(&mem);
        vm.set_file_host(mem);
        let gui = torque::gui::register(&mut vm);
        let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        let sent = DatagramQueue::default();
        let incoming = DatagramQueue::default();
        {
            let st = state.borrow();
            let mut net = st.net.borrow_mut();
            net.policy = NetPolicy::Loopback;
            net.socket = Some(Box::new(MemoryDatagrams {
                sent: Rc::clone(&sent),
                incoming: Rc::clone(&incoming),
                local_port: CLIENT_ADDR.port(),
            }));
        }
        // The engine's natives on ActionMap; the class itself is the game's to declare (TGE's is a plain SimObject).
        vm.register_class("ActionMap", "SimObject", &[]);
        torque::client::actionmap::register(&mut vm, &state);
        torque::client::screen::register(&mut vm, &gui, &state);
        torque::client::conn::register(&mut vm, &state, &gui);
        vm.set_global("$World::MapCount", &maps.len().to_string());
        for (i, m) in maps.iter().enumerate() {
            vm.set_global(&format!("$World::MapTitle{i}"), &m.title);
            vm.set_global(&format!("$World::MapAbout{i}"), &m.about);
        }
        if !vm
            .exec_file(b"client.cs", false)
            .map_err(|e| format!("client.cs: {e}"))?
        {
            return Err("client.cs is not in the game folder".to_string());
        }
        vm.invoke(None, b"buildMenu", &[])
            .map_err(|e| format!("buildMenu: {e}"))?;
        let mut host = ClientHost::new(
            Server::new(vm),
            gui,
            state,
            Rc::new(EnginePass),
            draw_cache(),
        );
        host.set_windowed();
        Ok(LevelDemo {
            host,
            files,
            maps,
            server: None,
            link: Link { sent, incoming },
            scene: WorldScene::default(),
            server_lag: 0,
            playing: None,
            frame_dt: 0.0,
            server_log: Vec::new(),
        })
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

    /// Starts map `index`: its server, and the client joins it.
    pub fn start_map(&mut self, index: usize) -> Result<(), String> {
        let map = self.maps.get(index).ok_or("no such map")?.clone();
        self.leave();
        let mut server = WorldServer::start(&self.files, &map.path)?;
        self.server_log.extend(server.drain_log());
        self.server = Some(server);
        self.server_lag = 0;
        self.playing = Some(index);
        let address = SERVER_ADDR.to_string();
        self.host
            .server
            .vm_mut()
            .invoke(None, b"joinWorld", &[address.into_bytes()])
            .map_err(|e| format!("joinWorld: {e}"))?;
        Ok(())
    }

    /// Stops the server being played, if there is one.
    pub fn leave(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = server.server.finish();
        }
        self.link.sent.borrow_mut().clear();
        self.link.incoming.borrow_mut().clear();
        self.scene.clear();
        self.playing = None;
    }

    /// A global the scripts set for the host, taken (cleared).
    fn take_global(&mut self, name: &str) -> Option<i32> {
        let vm = self.host.server.vm_mut();
        let v = vm.global(name).filter(|v| !v.is_empty())?;
        vm.set_global(name, "");
        Some(atoi(&v))
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
                    self.link.carry(server);
                    server.tick();
                    self.link.carry(server);
                }
                self.server_log.extend(server.drain_log());
            }
            self.host.run(slice, |_| false);
            if let Some(server) = self.server.as_ref() {
                self.link.carry(server);
            }
        }
        if self.server_log.len() > 2000 {
            let cut = self.server_log.len() - 1000;
            self.server_log.drain(..cut);
        }
        if let Some(pick) = self.take_global("$World::Pick")
            && let Ok(index) = usize::try_from(pick)
            && let Err(e) = self.start_map(index)
        {
            eprintln!("example-level-demo: {e}");
        }
        if self.take_global("$World::Leave").is_some() {
            self.leave();
        }
    }

    /// Whether the canvas shows the world (the server said the map started: `$World::InWorld`).
    pub fn in_world(&self) -> bool {
        self.host
            .server
            .vm()
            .global("$World::InWorld")
            .is_some_and(|v| atoi(&v) != 0)
    }

    /// The client's world of ghosts, if it is connected.
    fn with_world<R>(&self, f: impl FnOnce(&TgeWorld) -> R) -> Option<R> {
        let st = self.host.client.borrow();
        let conn = st.conn.conn.as_ref()?;
        Some(f(&conn.world))
    }

    /// How many ghosts the client holds.
    pub fn ghost_count(&self) -> usize {
        self.with_world(|w| w.records.ghosts.len()).unwrap_or(0)
    }

    /// Where the client's ghost of the first Player is, as its last record put it (no frame needed).
    pub fn ghost_figure_position(&self) -> Option<[f32; 3]> {
        self.with_world(|w| {
            w.records.ghosts.values().find_map(|g| match g.pose() {
                Some(torque::ghost_world::world::Pose::Euler { position, .. })
                    if g.class == Some(torque::ghosts::TgeClass::Player) =>
                {
                    Some(position)
                }
                _ => None,
            })
        })
        .flatten()
    }

    /// The client's own figure as the last frame drew it (the first figure: the example has one player).
    pub fn figure(&self) -> Option<FigureView> {
        let index = self.scene.figure_indices().next()?;
        self.scene.figure(index)
    }

    /// Whether the camera is at the figure's eyes (`V`).
    pub fn first_person(&self) -> bool {
        self.host
            .server
            .vm()
            .global("$World::FirstPerson")
            .is_some_and(|v| atoi(&v) != 0)
    }

    /// Sets where the camera is (the shots' second picture).
    pub fn set_first_person(&mut self, on: bool) {
        let v = if on { "1" } else { "0" };
        self.host
            .server
            .vm_mut()
            .set_global("$World::FirstPerson", v);
    }

    /// Adds `seconds` to the time the next frame covers (the clock's steps since the last frame).
    pub fn add_frame_time(&mut self, seconds: f32) {
        self.frame_dt += seconds;
    }

    /// The eye this frame: behind the figure or at its eyes; a fixed view of the middle of the map before there is one.
    pub fn eye(&self) -> Eye {
        match self.figure() {
            Some(f) if self.first_person() => {
                let at = [f.position[0], f.position[1], f.position[2] + EYE_HEIGHT];
                let (s, c) = f.pitch.sin_cos();
                let ahead = [at[0] + f.yaw.sin() * c, at[1] + f.yaw.cos() * c, at[2] - s];
                with_fov(look_at(at, ahead))
            }
            Some(f) => with_fov(self.third_person(&f)),
            None => with_fov(look_at([0.0, -30.0, 18.0], [0.0, 0.0, 0.0])),
        }
    }

    fn third_person(&self, f: &FigureView) -> Eye {
        let target = [
            f.position[0],
            f.position[1],
            f.position[2] + CAMERA_TARGET_HEIGHT,
        ];
        let pitch = (CAMERA_PITCH + f.pitch * 0.6).clamp(-0.6, 1.2);
        let (s, c) = pitch.sin_cos();
        let back = [-f.yaw.sin() * c, -f.yaw.cos() * c, s];
        let mut distance = CAMERA_DISTANCE;
        for (min, max) in self.scene.cube_bounds() {
            if let Some(t) = ray_box(target, back, min, max)
                && t < distance
            {
                distance = (t - CAMERA_CLEARANCE).max(0.4);
            }
        }
        let at = [0, 1, 2].map(|i| target[i] + back[i] * distance);
        look_at(at, target)
    }

    /// An eye looking at the whole map from above and to its south (the shots' overview).
    pub fn overview_eye(&self) -> Eye {
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (lo, hi) in self.scene.cube_bounds() {
            for i in 0..3 {
                min[i] = min[i].min(lo[i]);
                max[i] = max[i].max(hi[i]);
            }
        }
        if min[0] > max[0] {
            return self.eye();
        }
        let centre = [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5);
        let size = (max[0] - min[0]).max(max[1] - min[1]);
        let at = [
            centre[0] + size * 0.15,
            centre[1] - size * 0.85,
            max[2].max(0.0) + size * 0.7,
        ];
        with_fov(look_at(at, [centre[0], centre[1], 0.0]))
    }

    /// The frame: the ghosts into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let dt = std::mem::take(&mut self.frame_dt);
        let objects = {
            let st = self.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.scene.objects(view, &self.files.kit, &conn.world, dt),
                None => Vec::new(),
            }
        };
        view.set_objects(objects);
        // The client's camera is the example's own: the GameTSCtrl draws the world whatever the connection's camera is.
        self.host.gui.0.borrow_mut().world_camera = true;
        (self.host.draw_frame(), self.eye())
    }
}

fn with_fov(mut eye: Eye) -> Eye {
    eye.fov = None;
    eye
}

/// Where the ray from `from` along the unit `dir` enters the box `min`..`max` (its distance), if it does ahead.
pub fn ray_box(from: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let (mut near, mut far) = (0.0f32, f32::MAX);
    for i in 0..3 {
        if dir[i].abs() < 1e-6 {
            if from[i] < min[i] || from[i] > max[i] {
                return None;
            }
            continue;
        }
        let a = (min[i] - from[i]) / dir[i];
        let b = (max[i] - from[i]) / dir[i];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    // A ray that starts inside a box (the figure's own floor) does not count.
    (near > 0.0).then_some(near)
}
