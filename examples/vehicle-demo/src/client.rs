//! The example's client and the server it hosts: a client host (script engine, GUI, action maps, TGE's connection half and its
//! server query) on its own UDP socket, whose canvas is the menu or the world's view, and while it hosts, a [`WorldServer`] on
//! the same clock.
//!
//! **The socket** ([`NetMode`], the engine's [`torque::app::hosting`]). [`NetMode::Loopback`] binds the client's socket to
//! `127.0.0.1` (the tests, a game for one); [`NetMode::Lan`] binds it as TGE binds one and talks to the local network. A game
//! hosted here binds this computer's LAN address (or `--bind`'s; in [`NetMode::Loopback`] the loopback address), and the hosting
//! client joins it at once. A game played alone ([`VehicleDemo::play_alone`]) binds no socket: its server runs here on an
//! in-memory link ([`GameClient::start_local`]), the client's socket set aside while it runs.
//!
//! **A step** ([`VehicleDemo::step`]): the keys in a seat, then the engine's client step ([`GameClient::step`]: the hosted
//! server's ticks, then the client's); then what the scripts asked of the host: a game played alone (`$World::AloneAsked`), a
//! game hosted (`$World::HostAsked`) or the world left (`$World::LeaveAsked`: the server stops).
//!
//! **In a seat.** The keys set script globals ($Drive::Throttle, $Drive::Steer); before each step the driver's client turns them
//! into the move's globals ([`VehicleDemo::drive_input`]): the throttle is the move's forward or backward action; the
//! steering, a TGE WheeledVehicle's sum of the moves' yaws, is moved toward the held side's full lock (or back to the middle
//! with no key held) at [`STEER_SECONDS`] a full lock, by a yaw of the difference to where the client predicts it now. Space
//! holds the jump trigger, which the demo's Player hands to the buggy it drives: the brake ([`crate::seats`]). The moves made at
//! the wheel hold [`crate::seats::DRIVE_TRIGGER`] (only their yaw steers); none other does. A passenger's
//! keys move nothing.
//!
//! **A frame** ([`VehicleDemo::frame`]): the own figure where the client predicts it, the buggy where the client runs it, the
//! canvas drawn, the hint line, and the eye: on foot behind the figure (or at its eyes, `Tab`); in a seat behind and above the
//! buggy, turned round it by the mouse, kept above the ground, or (`Tab`) at the seated figure's eyes, looking along the buggy
//! turned by the mouse as far as a head turns ([`VehicleDemo::seated_eye_point`]: just ahead of the face, so the head does not
//! fill the view while the body, the hands on the wheel and the buggy are drawn). `Tab` eases the camera from one to the other
//! ([`CameraBlend`], kept above the ground as the camera behind is); the own figure on foot is hidden only once the camera has
//! arrived at its eyes. Getting in or out cuts. The wheel moves each camera behind (on foot, behind the buggy) in or out within its
//! own limits ([`FOOT_ZOOM`], [`DRIVE_ZOOM`]), eased ([`Zoom`]); in the first person it does nothing.

use std::ops::{Deref, DerefMut};

use std::net::Ipv4Addr;
use std::rc::Rc;

use torque::app::camera_blend::CameraBlend;
use torque::app::client::{ClientBuilder, ClientSetup, GameClient};
use torque::app::hosting::{join_address, lan_bind_address, register_join_natives};
use torque::app::lights::WorldLights;
use torque::app::orbit::Zoom;
use torque::app::terrain::Terrain;
use torque::app::view::{Daylight, WorldView};
use torque::app::{App, Host};
use torque::ghosts::TgeClass;
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::source::{Eye, SceneSource as _};

use crate::files::{GameFiles, TERRAIN_TEXTURE, TERRAIN_TILE};
use crate::scene::WorldScene;
use crate::seats::SeatedPlayerKind;
use crate::server::{ALONE_ADDR, DEFAULT_PORT, ServerOptions, WorldServer};
use torque::app::ghosts::OwnView;
pub use torque::app::hosting::NetMode;
use torque_example_common::prediction::CubeClient;

/// How a client is made.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub net: NetMode,
    /// The player's name (the menu's field starts with it).
    pub name: String,
    /// The UDP port number the LAN list asks and a hosted game binds (in [`NetMode::Loopback`] a hosted game takes any free one).
    pub port: u16,
    /// The address a hosted game binds (the loopback address: this computer's LAN address).
    pub host_address: Ipv4Addr,
}

impl Default for ClientOptions {
    fn default() -> ClientOptions {
        ClientOptions {
            net: NetMode::Lan,
            name: "Player".to_string(),
            port: DEFAULT_PORT,
            host_address: Ipv4Addr::LOCALHOST,
        }
    }
}

/// How the program starts: on the menu, or in a game at once (`--solo`, `--host`, `--join <address>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Menu,
    Alone,
    Host,
    Join(String),
}

impl Start {
    /// The start the flags ask for; more than one game is an error.
    pub fn from_flags(solo: bool, host: bool, join: Option<String>) -> Result<Start, String> {
        match (solo, host, join) {
            (false, false, None) => Ok(Start::Menu),
            (true, false, None) => Ok(Start::Alone),
            (false, true, None) => Ok(Start::Host),
            (false, false, Some(address)) => Ok(Start::Join(address)),
            _ => Err("--solo, --host and --join each start a game: give one of them".to_string()),
        }
    }

    /// Starts it on `game`.
    pub fn apply(self, game: &mut VehicleDemo) -> Result<(), String> {
        match self {
            Start::Menu => Ok(()),
            Start::Alone => game.play_alone(),
            Start::Host => game.host_game(),
            Start::Join(address) => {
                let address = torque::app::hosting::resolve_join_address(&address)?;
                game.join(&address.to_string())
            }
        }
    }
}

/// The client's address on the in-memory link of a game played alone.
pub const ALONE_CLIENT: std::net::SocketAddrV4 =
    std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_000);

/// The camera on foot: how far behind the figure, how high above its feet it looks, and how much it looks down.
pub const CAMERA_DISTANCE: f32 = 5.5;
pub const CAMERA_TARGET_HEIGHT: f32 = 1.7;
pub const CAMERA_PITCH: f32 = 0.3;
/// The camera in a seat: how far from the buggy, how high above its origin it looks, how much it looks down, how fast it
/// swings round behind the buggy as it turns (per second), and the least it keeps above the ground.
pub const DRIVE_DISTANCE: f32 = 8.0;
/// The nearest and farthest the mouse wheel takes the camera on foot and the one behind the buggy ([`Zoom`]).
pub const FOOT_ZOOM: (f32, f32) = (1.5, 12.0);
pub const DRIVE_ZOOM: (f32, f32) = (3.0, 20.0);
pub const DRIVE_TARGET_HEIGHT: f32 = 1.4;
pub const DRIVE_PITCH: f32 = 0.3;
pub const DRIVE_FOLLOW: f32 = 4.0;
pub const DRIVE_CLEARANCE: f32 = 0.8;
/// Seconds from the middle to full lock.
pub const STEER_SECONDS: f32 = 0.4;
/// A tick of the client's moves (seconds).
const TICK_SECONDS: f32 = 0.032;
/// How near the buggy the figure must be for the hint to offer a ride (units).
pub const HINT_REACH: f32 = 7.0;
/// The seated first-person view: how far the head turns each way and looks down and up (radians), and how far ahead of the
/// figure's `eye` node the view is (clear of the face).
pub const HEAD_TURN: f32 = 1.75;
/// How far the seated head looks down at rest (radians): the hood, the hands and the road ahead in view.
pub const HEAD_REST: f32 = 0.45;
pub const HEAD_DOWN: f32 = 1.05;
pub const HEAD_UP: f32 = 1.05;
/// How far the camera in a seat looks up (negative) and down at most (radians): above the buggy, short of straight down.
pub const DRIVE_PITCH_RANGE: (f32, f32) = (-0.5, 1.3);
pub const EYE_AHEAD: f32 = 0.12;

/// The predicted state of the buggy the client drives that the keys at the wheel read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Driven {
    pub steering: f32,
    pub max_steering: f32,
    pub velocity: [f32; 3],
    pub forward: [f32; 3],
}

/// The example's client (the engine's [`GameClient`], which it derefs to: its host, its globals, the server it hosts and their
/// logs) and the vehicle demo's own parts.
pub struct VehicleDemo {
    pub client: GameClient<WorldServer>,
    pub files: GameFiles,
    pub options: ClientOptions,
    pub terrain: Terrain,
    scene: WorldScene,
    /// The daylight the view was last given (the map's Sky over the default).
    daylight: Daylight,
    /// The yaw the camera in a seat looks along (it follows the buggy's heading).
    drive_yaw: Option<f32>,
    /// The hint line last written.
    hint: String,
    /// The seated figure's `eye` node in the buggy's space, by seat.
    seat_eyes: [[f32; 3]; 2],
    /// The headlights and the day of the world ([`WorldLights`]).
    lights: WorldLights,
    /// The camera's ease between first and third person, and the mode it last eased to: (first person, seated).
    blend: CameraBlend,
    camera_mode: Option<(bool, bool)>,
    /// The wheel's distances of the camera on foot and the one behind the buggy, each its own.
    foot_zoom: Zoom,
    drive_zoom: Zoom,
}

impl Deref for VehicleDemo {
    type Target = GameClient<WorldServer>;

    fn deref(&self) -> &GameClient<WorldServer> {
        &self.client
    }
}

impl DerefMut for VehicleDemo {
    fn deref_mut(&mut self) -> &mut GameClient<WorldServer> {
        &mut self.client
    }
}

impl VehicleDemo {
    /// The client over `files`: `client.cs` run, the menu showing.
    pub fn new(files: GameFiles, options: ClientOptions) -> Result<VehicleDemo, String> {
        let terrain = crate::files::terrain(&files)?;
        let mut b = ClientBuilder::new(ClientSetup {
            files: Some(files.mem_files()),
            game_classes: true,
            client_game: Some(Rc::new(CubeClient::with_terrain(
                files.mem_files(),
                Some(&terrain),
                Some(Rc::new(SeatedPlayerKind)),
            ))),
            // The list of games measures its text as it lays out: its fonts are the drawing's.
            layout_fonts: true,
            net: options.net.client_net(),
            server_query: true,
        })?;
        register_join_natives(&mut b.vm, &b.state);
        b.vm.set_global("$World::Port", &options.port.to_string());
        b.vm.set_global("$Pref::Player::Name", &options.name);
        b.exec("client.cs")?;
        Ok(VehicleDemo {
            client: b.build(),
            files,
            options,
            terrain,
            scene: WorldScene::default(),
            daylight: Daylight::default(),
            drive_yaw: None,
            hint: String::new(),
            seat_eyes: [0, 1].map(torque_example_assets::buggy::seated_eye),
            lights: WorldLights::default(),
            blend: CameraBlend::default(),
            camera_mode: None,
            foot_zoom: Zoom::new(CAMERA_DISTANCE, FOOT_ZOOM.0, FOOT_ZOOM.1),
            drive_zoom: Zoom::new(DRIVE_DISTANCE, DRIVE_ZOOM.0, DRIVE_ZOOM.1),
        })
    }

    /// Starts a game here: its server on this computer's LAN address (in [`NetMode::Loopback`] the loopback address, any free
    /// port), named after the player, and the client joins it.
    pub fn host_game(&mut self) -> Result<(), String> {
        self.stop_hosting();
        let loopback = self.options.net == NetMode::Loopback;
        let server = WorldServer::start(
            &self.files,
            &ServerOptions {
                address: lan_bind_address(self.options.net, self.options.host_address),
                port: if loopback { 0 } else { self.options.port },
                name: format!("{}'s game", self.global("$Pref::Player::Name")),
            },
        )?;
        let bound = server.address().ok_or("the server has no socket")?;
        self.client.start_hosting(server);
        self.call("connectTo", &[join_address(bound).to_string().as_str()])?;
        Ok(())
    }

    /// Starts a game here that nobody else can join (module docs), and the client joins it.
    pub fn play_alone(&mut self) -> Result<(), String> {
        self.stop_hosting();
        let name = format!("{}'s game", self.global("$Pref::Player::Name"));
        let server = WorldServer::start_alone(&self.files, &name)?;
        self.client.start_local(server, ALONE_CLIENT, ALONE_ADDR);
        self.call("connectTo", &[ALONE_ADDR.to_string().as_str()])?;
        Ok(())
    }

    /// Joins the game at `address` (`a.b.c.d:port`).
    pub fn join(&mut self, address: &str) -> Result<(), String> {
        self.stop_hosting();
        self.call("connectTo", &[address]).map(|_| ())
    }

    /// Stops the hosted server, if there is one.
    pub fn stop_hosting(&mut self) {
        self.client.stop_hosting();
        self.scene.clear();
        self.blend.reset();
        self.camera_mode = None;
    }

    /// A world view on `gpu`, the terrain its ground.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        let mut view = WorldView::new(gpu);
        view.set_terrain(
            &self.files.kit,
            &self.terrain,
            TERRAIN_TEXTURE,
            TERRAIN_TILE,
        );
        view
    }

    /// Whether the canvas shows the world (the server said the map started: `$World::InWorld`).
    pub fn in_world(&self) -> bool {
        self.flag("$World::InWorld")
    }

    /// The seat the figure is in (the server said so: `$Drive::Seat`): 0 the driver's, 1 the passenger's; none on foot.
    pub fn seat(&self) -> Option<u32> {
        let seat = self.global("$Drive::Seat");
        seat.trim().parse::<u32>().ok().filter(|_| self.in_world())
    }

    /// Whether the figure is at the wheel.
    pub fn driving(&self) -> bool {
        self.seat() == Some(0)
    }

    /// Whether the figure is in a seat.
    pub fn seated(&self) -> bool {
        self.seat().is_some()
    }

    /// Whether the camera on foot is at the figure's eyes (`V`).
    pub fn first_person(&self) -> bool {
        self.flag("$World::FirstPerson")
    }

    /// Puts the camera there at once, without easing (the shots).
    pub fn set_first_person(&mut self, on: bool) {
        self.set_global("$World::FirstPerson", if on { "1" } else { "0" });
        self.camera_mode = Some((on, self.seated()));
        self.blend.reset();
    }

    /// The scene of the client's ghosts as the last frame left it (the buggy's dust and tracks).
    pub fn scene(&self) -> &WorldScene {
        &self.scene
    }

    /// The client's own figure as the last frame drew it.
    pub fn figure(&self) -> Option<OwnView> {
        self.scene.own()
    }

    /// The buggy the client's figure is in, else the one nearest it (each player has one), else the first.
    pub fn buggy_ghost(&self) -> Option<u32> {
        self.with_world(|w| {
            let r = &w.records;
            let at = |i: u32| {
                r.ghosts
                    .get(&i)
                    .and_then(|g| g.pose())
                    .and_then(|p| match p {
                        torque::ghost_world::world::Pose::Euler { position, .. }
                        | torque::ghost_world::world::Pose::Quat { position, .. } => Some(position),
                        _ => None,
                    })
            };
            let own = r.control.object;
            if let Some((on, _)) = own.and_then(|o| r.mount_of(o)) {
                return Some(on);
            }
            let me = self
                .scene
                .own()
                .map(|f| f.position)
                .or_else(|| own.and_then(at));
            r.ghosts
                .iter()
                .filter(|(_, g)| g.class == Some(TgeClass::WheeledVehicle))
                .map(|(&i, _)| {
                    let d = match (me, at(i)) {
                        (Some(a), Some(b)) => (a[0] - b[0]).hypot(a[1] - b[1]),
                        _ => f32::MAX,
                    };
                    (d, i)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                .map(|(_, i)| i)
        })
        .flatten()
    }

    /// The buggy's wheels' contact points as the client draws them this frame (the ones on the ground).
    pub fn wheel_contacts(&self) -> Vec<[f32; 3]> {
        self.buggy_ghost()
            .map_or_else(Vec::new, |g| self.wheel_contacts_of(g))
    }

    /// [`VehicleDemo::wheel_contacts`] of the buggy whose ghost is `buggy`.
    pub fn wheel_contacts_of(&self, buggy: u32) -> Vec<[f32; 3]> {
        self.with_world(|w| {
            w.records
                .vehicle_look
                .get(&buggy)
                .map_or_else(Vec::new, |l| {
                    l.wheels
                        .iter()
                        .filter(|w| w.fitted && w.contact)
                        .map(|w| w.point)
                        .collect()
                })
        })
        .unwrap_or_default()
    }

    /// Where the client sits the buggy's `steering` thread (0 full right, 0.5 straight ahead, 1 full left).
    pub fn steering_place(&self) -> Option<f32> {
        let buggy = self.buggy_ghost()?;
        self.with_world(|w| torque_render_scene::world::steering_place(&w.records, buggy))
            .flatten()
    }

    /// The buggy's fitted wheels as the client draws them this frame: each one's spin (turns, 0 to 1) and whether it is in
    /// contact; none while the client does not run the buggy.
    pub fn wheel_spins(&self) -> Vec<(f32, bool)> {
        self.buggy_ghost()
            .map_or_else(Vec::new, |g| self.wheel_spins_of(g))
    }

    /// [`VehicleDemo::wheel_spins`] of the buggy whose ghost is `buggy`.
    pub fn wheel_spins_of(&self, buggy: u32) -> Vec<(f32, bool)> {
        self.with_world(|w| {
            w.records
                .vehicle_look
                .get(&buggy)
                .map(|l| {
                    l.wheels
                        .iter()
                        .filter(|w| w.fitted)
                        .map(|w| (w.apos, w.contact))
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
    }

    /// Whether the client plays a script thread on the buggy (its roof lamps lit while the headlight is on; a stopped one is
    /// back at its start, the lamps out).
    pub fn roof_lamps_lit(&self) -> bool {
        let Some(buggy) = self.buggy_ghost() else {
            return false;
        };
        self.with_world(|w| {
            w.records.threads.get(&buggy).is_some_and(|slots| {
                slots
                    .iter()
                    .flatten()
                    .any(|t| t.state == torque::ghost_world::threads::ThreadState::Play)
            })
        })
        .unwrap_or(false)
    }

    /// What the client's ghost of its own figure is mounted on (a ghost and its node), as the figure's record says.
    pub fn figure_mount(&self) -> Option<(u32, u32)> {
        self.with_world(|w| w.records.mount_of(w.records.control.object?))
            .flatten()
    }

    /// The figures' ghosts mounted on the buggy and their seats.
    pub fn riders(&self) -> Vec<(u32, u32)> {
        let Some(buggy) = self.buggy_ghost() else {
            return Vec::new();
        };
        self.with_world(|w| {
            w.records
                .ghosts
                .iter()
                .filter(|(_, g)| g.class == Some(TgeClass::Player))
                .filter_map(|(&g, _)| match w.records.mount_of(g) {
                    Some((on, node)) if on == buggy => Some((g, node)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The buggy where the client draws it (where it runs it, else where its record put it): its placement.
    pub fn buggy_pose(&self) -> Option<torque_render_scene::math::Mat> {
        let index = self.buggy_ghost()?;
        let pose = self
            .with_world(|w| {
                w.vehicle_render
                    .get(&index)
                    .copied()
                    .or_else(|| w.records.ghosts.get(&index).and_then(|g| g.pose()))
            })
            .flatten()?;
        pose_matrix(&pose, None).map(|(m, _)| m)
    }

    /// The buggy's wheels' spring extensions (0 pressed up to 1 hanging) as the client runs it.
    pub fn buggy_extensions(&self) -> Vec<f32> {
        let Some(index) = self.buggy_ghost() else {
            return Vec::new();
        };
        self.with_world(|w| {
            w.vehicle_look
                .get(&index)
                .map(|l| l.wheels.iter().map(|w| w.extension).collect())
        })
        .flatten()
        .unwrap_or_default()
    }

    /// The buggy's speed (units a second) as the client runs it.
    pub fn buggy_speed(&self) -> f32 {
        let Some(index) = self.buggy_ghost() else {
            return 0.0;
        };
        self.with_world(|w| w.vehicle_look.get(&index).map(|l| l.velocity))
            .flatten()
            .map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
    }

    /// The buggy the client drives, as it predicts it.
    pub fn driven(&self) -> Option<Driven> {
        let st = self.client.host.client.borrow();
        let conn = st.conn.conn.as_ref()?;
        let v = conn.predict.predicted.as_ref()?.vehicle.as_ref()?;
        let m = v.v.s.obj_to_world;
        Some(Driven {
            steering: v.v.s.steering[0],
            max_steering: v.data.base.max_steering_angle,
            velocity: v.v.s.rigid.lin_vel,
            forward: [m[1], m[5], m[9]],
        })
    }

    /// The keys at the wheel made into the next move's globals (module docs); `ms` the time the step covers.
    pub fn drive_input(&mut self, ms: u32) {
        let at_wheel = self.driving().then(|| self.driven()).flatten();
        self.hold_trigger(crate::seats::DRIVE_TRIGGER, at_wheel.is_some());
        let Some(d) = at_wheel else {
            if self.driving() {
                self.set_global("$mvYaw", "0");
            }
            return;
        };
        let throttle = self.number("$Drive::Throttle").clamp(-1.0, 1.0);
        let steer = self.number("$Drive::Steer").clamp(-1.0, 1.0);
        self.set_global("$mvForwardAction", &throttle.max(0.0).to_string());
        self.set_global("$mvBackwardAction", &(-throttle).max(0.0).to_string());
        // The steering: a yaw of the difference to the held side's lock (A steers left: the steering's negative side), at most
        // a tick's share of the rate: the next move takes the global as it stands when its tick comes (a step shorter than a tick
        // sets it again, it does not add), and the predicted steering moves a tick at a time.
        let _ = ms;
        let target = -steer * d.max_steering;
        let rate = d.max_steering / STEER_SECONDS * TICK_SECONDS;
        let turn = (target - d.steering).clamp(-rate, rate);
        self.set_global("$mvYaw", &turn.to_string());
    }

    /// Holds trigger `i` on the next moves (an odd `$mvTriggerCount<i>`) or lets it go.
    fn hold_trigger(&mut self, i: usize, on: bool) {
        let name = format!("$mvTriggerCount{i}");
        let count = self.number(&name) as i64;
        if (count % 2 == 1) != on {
            self.set_global(&name, &(count + 1).to_string());
        }
    }

    /// `ms` of the clock (module docs).
    pub fn step(&mut self, ms: u32) {
        self.drive_input(ms);
        self.client.step(ms);
        let asked = if self.take_flag("$World::AloneAsked") {
            Some(self.play_alone())
        } else if self.take_flag("$World::HostAsked") {
            Some(self.host_game())
        } else {
            None
        };
        if let Some(Err(e)) = asked {
            eprintln!("vehicle-demo: {e}");
            if let Some(status) = self.vm().objects().find(b"MenuStatus") {
                let text = format!("Could not start the game: {e}");
                let _ = self
                    .vm_mut()
                    .call_method(status, b"setText", &[text.into_bytes()]);
            }
        }
        if self.take_flag("$World::LeaveAsked") {
            self.stop_hosting();
        }
    }

    /// The eye this frame (module docs), `dt` seconds after the last.
    pub fn eye(&mut self, dt: f32) -> Eye {
        if self.seated()
            && let Some(m) = self.buggy_pose()
        {
            if self.first_person()
                && let Some(e) = self.seated_eye(&m)
            {
                return with_fov(e);
            }
            return with_fov(self.drive_eye(&m, dt));
        }
        self.drive_yaw = None;
        match self.figure() {
            Some(f) if self.first_person() => {
                let at = f.eye;
                let (s, c) = f.pitch.sin_cos();
                let ahead = [at[0] + f.yaw.sin() * c, at[1] + f.yaw.cos() * c, at[2] - s];
                with_fov(look_at(at, ahead))
            }
            Some(f) => with_fov(self.foot_eye(&f)),
            None => with_fov(look_at([-12.0, -14.0, 28.0], [0.0, 0.0, 20.0])),
        }
    }

    /// The eye drawn this frame: [`VehicleDemo::eye`], eased from the last mode's after a switch (module docs).
    pub fn drawn_eye(&mut self, dt: f32) -> Eye {
        let target = self.eye(dt);
        let terrain = &self.terrain;
        self.blend.eye(target, |mut p| {
            if let Some(ground) = terrain.height(p[0], p[1]) {
                p[2] = p[2].max(ground + DRIVE_CLEARANCE);
            }
            p
        })
    }

    /// The distance of the third-person camera the mode has (on foot or behind the buggy), as the wheel left it.
    pub fn camera_distance(&self) -> f32 {
        if self.seated() {
            self.drive_zoom.distance()
        } else {
            self.foot_zoom.distance()
        }
    }

    /// The wheel's notches since the last frame (`$World::Zoom`, 120 a notch) to the third-person camera of the mode (none in
    /// the first person), then `dt` seconds of each camera's easing.
    fn zoom(&mut self, dt: f32) {
        let wheel = self.number("$World::Zoom");
        if wheel != 0.0 {
            self.set_global("$World::Zoom", "0");
            if !self.first_person() {
                let zoom = if self.seated() {
                    &mut self.drive_zoom
                } else {
                    &mut self.foot_zoom
                };
                zoom.notches(wheel / 120.0);
            }
        }
        self.foot_zoom.advance(dt);
        self.drive_zoom.advance(dt);
    }

    /// Whether the camera is easing between modes.
    pub fn camera_blending(&self) -> bool {
        !self.blend.arrived()
    }

    /// The seated figure's look in the buggy's space: along the buggy turned by the mouse (`$Drive::OrbitYaw`,
    /// `$Drive::OrbitPitch`) within a head's turn.
    fn seated_look(&self) -> [f32; 3] {
        let yaw = self.number("$Drive::OrbitYaw").clamp(-HEAD_TURN, HEAD_TURN);
        let down = (HEAD_REST + self.number("$Drive::OrbitPitch")).clamp(-HEAD_UP, HEAD_DOWN);
        let (s, c) = down.sin_cos();
        [yaw.sin() * c, yaw.cos() * c, -s]
    }

    /// Where the seated first-person view is in the buggy's space: the seated figure's `eye` node, [`EYE_AHEAD`] along the
    /// look, level.
    pub fn seated_eye_local(&self) -> Option<[f32; 3]> {
        let eye = *self.seat_eyes.get(self.seat()? as usize)?;
        let look = self.seated_look();
        let flat = look[0].hypot(look[1]).max(1e-3);
        Some([
            eye[0] + look[0] / flat * EYE_AHEAD,
            eye[1] + look[1] / flat * EYE_AHEAD,
            eye[2],
        ])
    }

    /// Where the seated first-person view is in the world: [`VehicleDemo::seated_eye_local`] on the buggy as the client draws it.
    pub fn seated_eye_point(&self) -> Option<[f32; 3]> {
        Some(self.buggy_pose()?.mul_p(self.seated_eye_local()?))
    }

    fn seated_eye(&self, m: &torque_render_scene::math::Mat) -> Option<Eye> {
        let at = m.mul_p(self.seated_eye_local()?);
        let look = m.mul_v(self.seated_look());
        Some(look_at(at, [0, 1, 2].map(|i| at[i] + look[i])))
    }

    fn foot_eye(&self, f: &OwnView) -> Eye {
        let target = [
            f.position[0],
            f.position[1],
            f.position[2] + CAMERA_TARGET_HEIGHT,
        ];
        let pitch = (CAMERA_PITCH + f.pitch * 0.6).clamp(-0.6, 1.2);
        self.orbit(target, f.yaw, pitch, self.foot_zoom.distance())
    }

    fn drive_eye(&mut self, m: &torque_render_scene::math::Mat, dt: f32) -> Eye {
        let p = m.column(3);
        let forward = m.column(1);
        let heading = forward[0].atan2(forward[1]);
        // The camera swings round behind the buggy as it turns, along the shorter way.
        let yaw = match self.drive_yaw {
            Some(y) => {
                let mut d = heading - y;
                d -= std::f32::consts::TAU * (d / std::f32::consts::TAU).round();
                y + d * (1.0 - (-DRIVE_FOLLOW * dt).exp())
            }
            None => heading,
        };
        self.drive_yaw = Some(yaw);
        let target = [p[0], p[1], p[2] + DRIVE_TARGET_HEIGHT];
        let orbit_yaw = self.number("$Drive::OrbitYaw");
        let pitch = (DRIVE_PITCH + self.number("$Drive::OrbitPitch"))
            .clamp(DRIVE_PITCH_RANGE.0, DRIVE_PITCH_RANGE.1);
        self.orbit(target, yaw + orbit_yaw, pitch, self.drive_zoom.distance())
    }

    /// An eye `distance` behind `target` along `yaw`, raised by `pitch`, looking at it, kept above the ground.
    fn orbit(&self, target: [f32; 3], yaw: f32, pitch: f32, distance: f32) -> Eye {
        let (s, c) = pitch.sin_cos();
        let back = [-yaw.sin() * c, -yaw.cos() * c, s];
        let mut at = [0, 1, 2].map(|i| target[i] + back[i] * distance);
        if let Some(ground) = self.terrain.height(at[0], at[1]) {
            at[2] = at[2].max(ground + DRIVE_CLEARANCE);
        }
        look_at(at, target)
    }

    /// The daylight of the map's Sky as the client's ghost of it has it: its fog color, fog distance and visible distance
    /// over the [`Daylight`] default; None before a Sky is ghosted.
    pub fn sky_daylight(&self) -> Option<Daylight> {
        let sky = self.with_world(|w| w.records.sky())??;
        let mut d = Daylight::default();
        if let Some([visible, fog]) = sky.distances {
            d.visible = visible;
            d.fog_start = fog;
        }
        if let Some(main) = sky.main {
            d.fog = main.fog_color;
        }
        Some(d)
    }

    /// The hint line for this frame.
    pub fn hint_text(&self) -> String {
        if !self.in_world() {
            return String::new();
        }
        // A note the day's keys left (the time and its speed, or why it did not change) for a moment.
        let note = self.global("$World::DayNote");
        if !note.is_empty() {
            return note;
        }
        if self.seated() {
            return format!("E to get out      {:.0} km/h", self.buggy_speed() * 3.6);
        }
        let (Some(f), Some(m)) = (self.figure(), self.buggy_pose()) else {
            return String::new();
        };
        let j = m.column(3);
        let near = (f.position[0] - j[0]).hypot(f.position[1] - j[1]) < HINT_REACH;
        if near {
            "Click the buggy to get in".to_string()
        } else {
            String::new()
        }
    }

    /// The frame's motion `dt` seconds after the last, with nothing drawn (a frame's first half): the client's prediction
    /// drawn to the frame, the figures posed, the buggy's dust and tracks laid.
    pub fn update_scene(&mut self, dt: f32) {
        self.client.predict_frame();
        let mode = (self.first_person(), self.seated());
        match self.camera_mode.replace(mode) {
            Some(last) if last.1 != mode.1 => self.blend.reset(),
            Some(last) if last.0 != mode.0 => self.blend.switch(),
            _ => {}
        }
        self.blend.advance(dt);
        self.zoom(dt);
        let hide = mode.0 && !mode.1 && self.blend.arrived();
        let mut st = self.client.host.client.borrow_mut();
        match st.conn.conn.as_mut() {
            Some(conn) => {
                let conn = &mut **conn;
                // The camera is the example's own: the figure is seen unless the view is at its eyes.
                conn.world.hide_control = hide;
                self.scene.update(
                    &self.files.kit,
                    &mut conn.world,
                    &conn.predict.ghost_players,
                    &self.terrain,
                    dt,
                );
                self.lights.update(&conn.world.records, dt);
            }
            None => {
                self.scene.clear();
                self.lights.clear();
            }
        }
    }

    /// Where the day's sun (or moon) is as the client runs the server's TimeOfDay, towards it; none before one is ghosted.
    pub fn sun(&self) -> Option<[f32; 3]> {
        self.lights.sun()
    }

    /// Whether the buggy's headlight is on, as the client's ghost of it says; none before it is ghosted.
    pub fn headlight_on(&self) -> Option<bool> {
        let buggy = self.buggy_ghost()?;
        self.with_world(|w| {
            w.records
                .of_class(TgeClass::SpotLight)
                .filter(|(i, _)| w.records.mount_of(*i).is_some_and(|m| m.0 == buggy))
                .find_map(|(_, g)| match g.record.as_ref() {
                    Some(torque::ghosts::TgeRecord::SpotLight(r)) => r.enabled,
                    _ => None,
                })
        })
        .flatten()
    }

    /// The spot lights the client draws (each on, where it rides).
    pub fn spot_lights(&mut self) -> Vec<torque_render_scene::hd::HdSpot> {
        let st = self.client.host.client.borrow();
        match st.conn.conn.as_ref() {
            Some(conn) => self.lights.spots(&self.files.kit, &conn.world.records),
            None => Vec::new(),
        }
    }

    /// The frame: the ghosts into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        if let Some(d) = self.sky_daylight()
            && d != self.daylight
        {
            view.set_daylight(&d);
            self.daylight = d;
        }
        let dt = self.client.take_frame_time();
        self.update_scene(dt);
        let eye = self.drawn_eye(dt);
        let objects = {
            let st = self.client.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.scene.objects(view, &self.files.kit, &conn.world, &eye),
                None => Vec::new(),
            }
        };
        view.set_objects(objects);
        {
            let st = self.client.host.client.borrow();
            if let Some(conn) = st.conn.conn.as_ref() {
                self.lights
                    .apply(view, &self.files.kit, &conn.world.records);
            }
        }
        let hint = self.hint_text();
        if hint != self.hint {
            let _ = self.call("setHint", &[&hint]);
            self.hint = hint;
        }
        // The client's camera is the example's own: the GameTSCtrl draws the world whatever the connection's camera is.
        let host = &mut self.client.host;
        host.gui.0.borrow_mut().world_camera = true;
        (host.draw_frame(), eye)
    }
}

impl App for VehicleDemo {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "torque-rs vehicle demo".to_string()
    }

    fn log_name(&self) -> &str {
        "vehicle-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        VehicleDemo::view(self, gpu)
    }

    /// A step, and its time added to the next frame's.
    fn advance(&mut self, ms: u32) {
        self.step(ms);
        self.add_frame_time(ms as f32 / 1000.0);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        VehicleDemo::frame(self, view)
    }
}

fn with_fov(mut eye: Eye) -> Eye {
    eye.fov = None;
    eye
}
