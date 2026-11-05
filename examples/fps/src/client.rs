//! The example's client, and the server it hosts: a client host (script engine, GUI, action maps, TGE's connection half and its
//! server query) on its own UDP socket, whose canvas is the menu, the list of games or the arena's view, and while it hosts, an
//! [`ArenaServer`] on the same clock.
//!
//! **The socket** ([`NetMode`], the engine's [`torque::app::hosting`]). [`NetMode::Loopback`] binds the client's socket to `127.0.0.1` and reads only loopback datagrams (the tests,
//! a game for one); [`NetMode::Lan`] binds it as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1`
//! hears its LAN query) and talks to LAN addresses only, but for the one public address the player types into the join screen
//! and connects to (the engine's internet policy with a permit the typed address arms, see [`parse_join_address`]).
//!
//! **Hosting.** The menu's Host game offers a LAN game (the server binds this computer's LAN address, found from the system's
//! routing, or `--bind`'s; in [`NetMode::Loopback`] the host address) and a public one (`0.0.0.0`: every interface; players
//! outside the network need that UDP port forwarded to this computer and the firewall to allow it).
//!
//! **A step** ([`ArenaGame::step`]): the engine's client step ([`GameClient::step`]: the hosted server's ticks, then the
//! client's); then what the scripts asked of the host: a game hosted (`$Arena::HostAsked`: the server starts and the client
//! joins it over the loopback address) or the arena left (`$Arena::LeaveAsked`: the server stops).
//!
//! **A frame** ([`ArenaGame::frame`]): the client's own figure where the client predicts it, between its last two ticks
//! (`Player::interpolateTick`) and turned ahead by the mouse's waiting turn ([`torque_example_common::prediction`]), the other figures where the
//! client runs them between their records, the ghosts as the world view's objects ([`crate::scene`]), the canvas drawn, and
//! the eye at the own figure's eye, or behind it (`Tab`).
//!
//! **The camera** ([`ArenaGame::drawn_eye`]): at the own figure's eye, or with `Tab` ([`ArenaGame::third_person`]) behind it,
//! [`CAMERA_DISTANCE`] back along its look, [`CAMERA_SIDE`] to its right and [`CAMERA_LIFT`] up, looking the same way (the
//! crosshair stays on the shots' line far off), kept [`CAMERA_CLEARANCE`] in front of the first cube or interior wall between
//! (their boxes; an interior's walls in the client's container). Backed against a wall, with less than [`CAMERA_NEAREST`] left behind the eye, the view is drawn as
//! the first person's (the figure hidden, the pistol in view) until there is room again.
//! `Tab` eases the camera from one to the other ([`CameraBlend`], each eye of the ease kept in front of walls the same way);
//! the figure is hidden only once the camera has arrived at its eyes, and its pistol is drawn in its hand while it is seen.
//!
//! Credit: the frame between the last two ticks follows `Player::interpolateTick` and the client's `ProcessList` in Torque3D's
//! MIT-licensed `T3D/player.cpp` and `T3D/gameBase/processList.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015
//! Faust Logic, Inc. for `player.cpp`). The code here is this example's own.

use std::net::Ipv4Addr;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use torque::app::camera_blend::CameraBlend;
use torque::app::client::{ClientBuilder, ClientNet, ClientSetup, GameClient};
use torque::app::view::{Daylight, WorldView};
use torque::app::{App, Host};
use torque::client::conn::tge::TgeWorld as World;
use torque::client::net::{Datagrams, NetPolicy};
use torque::ghost_world::world::Pose;
use torque::gui::GuiDraws;
use torque::script::text::atoi;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::effects::{ScreenMarks, insert_over_world};
use crate::files::GameFiles;
use crate::scene::{ArenaScene, OwnLook, TextureFilter};
use crate::server::{ArenaServer, DEFAULT_PORT, ServerOptions};
pub use torque::app::hosting::{NetMode, parse_join_address};
use torque::app::hosting::{join_address, lan_bind_address, lan_reach, register_join_natives};
use torque_example_common::prediction::CubeClient;

/// The camera behind the figure: how far back along its look, how far above its eye, and the nearest it comes to a wall.
pub const CAMERA_DISTANCE: f32 = 3.5;
pub const CAMERA_SIDE: f32 = 0.6;
pub const CAMERA_LIFT: f32 = 0.45;
pub const CAMERA_CLEARANCE: f32 = 0.3;
/// The least room behind the eye the camera behind the figure needs; with less, the view is the first person's.
pub const CAMERA_NEAREST: f32 = 0.8;

/// What the camera behind the figure keeps clear of in the client's container: the interiors (the cubes by their boxes).
const CAMERA_MASK: u32 = torque::game::scene::interior::INTERIOR_TYPE_MASK;

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

/// The example's client (the engine's [`GameClient`], which it derefs to: its host, its globals, the server it hosts and
/// their logs) and the arena's own parts.
pub struct ArenaGame {
    pub client: GameClient,
    pub files: GameFiles,
    pub options: ClientOptions,
    scene: ArenaScene,
    /// The Sky's distances the view was given.
    sky: Option<(f32, f32)>,
    /// The screen's hurt and hit marks.
    pub marks: ScreenMarks,
    /// The camera's ease between the eye and behind the figure, and the mode it last eased to (behind).
    blend: CameraBlend,
    camera_mode: Option<bool>,
    /// The camera behind the figure had less than [`CAMERA_NEAREST`] of room last frame.
    cramped: bool,
}

impl Deref for ArenaGame {
    type Target = GameClient;

    fn deref(&self) -> &GameClient {
        &self.client
    }
}

impl DerefMut for ArenaGame {
    fn deref_mut(&mut self) -> &mut GameClient {
        &mut self.client
    }
}

impl ArenaGame {
    /// The client over `files`: `client.cs` run, the main menu showing.
    pub fn new(files: GameFiles, options: ClientOptions) -> Result<ArenaGame, String> {
        ArenaGame::build(files, options, None)
    }

    /// The client over `files` on `socket` (a game socket over WebSocket: a browser's, or a test's), its address rule
    /// the loopback one (the socket names its server by a stand-in loopback address); `options.net` is not used.
    pub fn with_socket(
        files: GameFiles,
        options: ClientOptions,
        socket: Box<dyn Datagrams>,
    ) -> Result<ArenaGame, String> {
        ArenaGame::build(files, options, Some(socket))
    }

    fn build(
        files: GameFiles,
        options: ClientOptions,
        socket: Option<Box<dyn Datagrams>>,
    ) -> Result<ArenaGame, String> {
        let net = if socket.is_some() {
            ClientNet::External
        } else {
            options.net.client_net()
        };
        let mut b = ClientBuilder::new(ClientSetup {
            files: Some(files.mem_files()),
            game_classes: true,
            client_game: Some(Rc::new(CubeClient::new(files.mem_files()))),
            // The controls that measure text as they lay out (the list of games) find their fonts where the drawing does.
            layout_fonts: true,
            net,
            server_query: true,
        })?;
        if let Some(socket) = socket {
            b.set_socket(socket, NetPolicy::Loopback);
        }
        register_join_natives(&mut b.vm, &b.state);
        b.vm.set_global("$Arena::Port", &options.port.to_string());
        b.exec("client.cs")?;
        b.vm.set_global("$Pref::Player::Name", &options.name);
        b.invoke("showMainMenu")?;
        Ok(ArenaGame {
            client: b.build(),
            files,
            options,
            scene: ArenaScene::new(),
            sky: None,
            marks: ScreenMarks::default(),
            blend: CameraBlend::default(),
            camera_mode: None,
            cramped: false,
        })
    }

    /// A world view on `gpu`.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    /// Starts a game here as `$Arena::HostScope` says (`public`: every interface; else the LAN): its server, and the client joins
    /// it.
    pub fn host_game(&mut self) -> Result<(), String> {
        self.stop_hosting();
        let public = self.global("$Arena::HostScope") == "public";
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
        // A harness ([`NetMode::Loopback`]) never opens a socket beyond the loopback address, whoever may join.
        let address = if public && self.options.net == NetMode::Lan {
            Ipv4Addr::UNSPECIFIED
        } else {
            self.lan_bind_address()
        };
        let server = ArenaServer::start(
            &self.files,
            &ServerOptions {
                address,
                port,
                name,
                mission,
                public,
            },
        )?;
        let bound = server.address().ok_or("the server has no socket")?;
        let join = join_address(bound).to_string();
        self.client.start_hosting(server);
        self.set_host_info(public, bound.port());
        self.vm_mut()
            .invoke(None, b"connectTo", &[join.into_bytes()])
            .map_err(|e| format!("connectTo: {e}"))?;
        Ok(())
    }

    /// The addresses other players on this computer's network join a hosted game by: the one `--bind` named, else this
    /// computer's own LAN addresses; none for a harness ([`NetMode::Loopback`]).
    fn lan_reach(&self) -> Vec<Ipv4Addr> {
        lan_reach(self.options.net, self.options.host_address)
    }

    /// The address a LAN game binds: the first of [`ArenaGame::lan_reach`], else the host address (loopback: only this computer
    /// can join).
    fn lan_bind_address(&self) -> Ipv4Addr {
        lan_bind_address(self.options.net, self.options.host_address)
    }

    /// The lines the view's chat box shows when the hosted game starts (`$Arena::HostInfo1` to `3`): where the others join, and
    /// for a public game what the router and the firewall need.
    fn set_host_info(&mut self, public: bool, port: u16) {
        let reach: Vec<String> = self
            .lan_reach()
            .iter()
            .take(2)
            .map(|ip| format!("{ip}:{port}"))
            .collect();
        let lines = if public {
            let first = if reach.is_empty() {
                format!("Public game on port {port}.")
            } else {
                format!("Public game. On this network: {}", reach.join(" or "))
            };
            [
                first,
                format!("Outside players: forward UDP {port} to this computer"),
                "and allow it through the firewall.".to_string(),
            ]
        } else if reach.is_empty() {
            [
                "Only this computer can join: no network found.".to_string(),
                String::new(),
                String::new(),
            ]
        } else {
            [
                format!(
                    "LAN game. Others on your network join {}",
                    reach.join(" or ")
                ),
                String::new(),
                String::new(),
            ]
        };
        for (i, line) in lines.iter().enumerate() {
            self.set_global(&format!("$Arena::HostInfo{}", i + 1), line);
        }
    }

    /// Replaces the client's socket (a browser's client opens a new game socket to join again).
    pub fn set_socket(&mut self, socket: Box<dyn Datagrams>) {
        let st = self.client.host.client.borrow();
        let mut n = st.net.borrow_mut();
        n.socket = Some(socket);
        n.policy = NetPolicy::Loopback;
    }

    /// Stops the hosted server, if there is one.
    pub fn stop_hosting(&mut self) {
        self.client.stop_hosting();
        self.scene.clear();
        self.blend.reset();
        self.camera_mode = None;
    }

    /// `ms` of the clock (see the module docs).
    pub fn step(&mut self, ms: u32) {
        self.client.step(ms);
        if self.take_flag("$Arena::HostAsked")
            && let Err(e) = self.host_game()
        {
            eprintln!("example-fps: {e}");
            let text = format!("Could not start the game: {e}");
            for name in [&b"MainStatus"[..], b"HostStatus"] {
                if let Some(status) = self.vm().objects().find(name) {
                    let _ =
                        self.vm_mut()
                            .call_method(status, b"setText", &[text.clone().into_bytes()]);
                }
            }
        }
        if self.take_flag("$Arena::LeaveAsked") {
            self.stop_hosting();
        }
    }

    /// Whether the canvas shows the arena (the server said the game started).
    pub fn in_game(&self) -> bool {
        self.flag("$Arena::InGame")
    }

    /// Whether the scripts asked to quit.
    pub fn quit_asked(&self) -> bool {
        self.flag("$Arena::QuitAsked")
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
        let Some((visible, fog)) = self.sky_distances() else {
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

    /// The frame's motion, with nothing drawn: the own figure where the client predicts it and the other figures moved on by the
    /// time since the last frame. Returns that time (seconds).
    pub fn view_frame(&mut self) -> f32 {
        let dt = self.client.take_frame_time();
        if self.in_game() {
            let hurts = crate::effects::count(&self.global("$Arena::HurtCount"));
            let from = crate::effects::point(&self.global("$Arena::HurtFrom"));
            let hits = crate::effects::count(&self.global("$Arena::Hits"));
            self.marks.update(hurts, from, hits, dt);
        } else {
            self.marks.clear();
        }
        let aiming = self.in_game() && self.flag("$Arena::Aiming");
        self.scene.update_aim(aiming, dt);
        let scale = self.scene.turn_scale();
        self.set_global("$Arena::TurnScale", &scale.to_string());
        self.client.predict_frame();
        let behind = self.in_game() && self.third_person();
        if self
            .camera_mode
            .replace(behind)
            .is_some_and(|m| m != behind)
        {
            self.blend.switch();
        }
        self.blend.advance(dt);
        // The figure is seen (and holds its pistol in its hand) unless the camera is at its eyes.
        let seen = (behind && !self.cramped) || !self.blend.arrived();
        let mut st = self.client.host.client.borrow_mut();
        match st.conn.conn.as_mut() {
            Some(conn) => {
                let conn = &mut **conn;
                if seen {
                    conn.world.hide_control = false;
                }
                let world: &mut World = &mut conn.world;
                self.scene
                    .update(&self.files.kit, world, &conn.predict.ghost_players, dt);
            }
            None => self.scene.clear(),
        }
        dt
    }

    /// Whether the camera is behind the figure (`Tab`: `$Arena::ThirdPerson`).
    pub fn third_person(&self) -> bool {
        self.flag("$Arena::ThirdPerson")
    }

    /// Whether the camera is easing between the eye and behind the figure.
    pub fn camera_blending(&self) -> bool {
        !self.blend.arrived()
    }

    /// The eye of the mode this frame: the figure's eye, or behind it (module docs).
    pub fn mode_eye(&self) -> Eye {
        let eye = self.eye();
        if !(self.in_game() && self.third_person() && self.scene.own().is_some()) {
            return eye;
        }
        let (sp, cp) = eye.pitch.sin_cos();
        let (sy, cy) = eye.yaw.sin_cos();
        let at = eye.position;
        let back = [
            at[0] - sy * cp * CAMERA_DISTANCE + cy * CAMERA_SIDE,
            at[1] - cy * cp * CAMERA_DISTANCE - sy * CAMERA_SIDE,
            at[2] + sp * CAMERA_DISTANCE + CAMERA_LIFT,
        ];
        Eye {
            position: self.kept_clear(at, back),
            ..eye
        }
    }

    /// The point `to`, brought [`CAMERA_CLEARANCE`] in front of the first cube or interior wall between `from` (the figure's
    /// eye) and it.
    fn kept_clear(&self, from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
        let d = [0, 1, 2].map(|k| to[k] - from[k]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if length < 1e-4 {
            return to;
        }
        let dir = d.map(|v| v / length);
        let mut st = self.client.host.client.borrow_mut();
        let Some(conn) = st.conn.conn.as_mut() else {
            return to;
        };
        let conn = &mut **conn;
        let mut first = crate::scene::cube_boxes(&conn.world.records)
            .into_iter()
            .filter_map(|(lo, hi)| torque_example_common::ray_box(from, dir, lo, hi))
            .fold(length, f32::min);
        if let Some(hit) = conn.predict.cast_ray(&conn.world, from, to, CAMERA_MASK) {
            first = first.min(hit.t * length);
        }
        if first >= length {
            return to;
        }
        let reach = (first - CAMERA_CLEARANCE).max(0.0);
        [0, 1, 2].map(|k| from[k] + dir[k] * reach)
    }

    /// The eye drawn this frame: [`ArenaGame::mode_eye`], eased from the last mode's after a switch.
    pub fn drawn_eye(&mut self) -> Eye {
        let target = self.mode_eye();
        let from = self.eye().position;
        let mut blend = self.blend;
        let eye = blend.eye(target, |p| self.kept_clear(from, p));
        self.blend = blend;
        let room = (0..3)
            .map(|k| (eye.position[k] - from[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        // A little more room to leave the first person than to take it, so a step at the edge does not flicker.
        let least = if self.cramped {
            CAMERA_NEAREST + 0.2
        } else {
            CAMERA_NEAREST
        };
        self.cramped = self.third_person() && blend.arrived() && room < least;
        if self.cramped {
            return self.eye();
        }
        eye
    }

    /// The frame: the ghosts into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        self.view_frame();
        let filter = TextureFilter::named(&self.global("$Arena::TextureFilter"));
        self.scene.set_texture_filter(filter);
        let objects = {
            let st = self.client.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.scene.objects(view, &self.files.kit, &conn.world),
                None => Vec::new(),
            }
        };
        view.set_objects(objects);
        self.apply_sky(view);
        // The example draws the world itself: the GameTSCtrl draws the world whatever the connection's camera is.
        let host = &mut self.client.host;
        host.gui.0.borrow_mut().world_camera = true;
        let mut draws = host.draw_frame();
        let eye = self.drawn_eye();
        if let Some(world) = draws.worlds.first() {
            let marks = self.marks.batches(world.rect, eye.position, eye.yaw);
            insert_over_world(&mut draws, marks);
        }
        (draws, eye)
    }
}

impl App for ArenaGame {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "torque-rs arena".to_string()
    }

    fn log_name(&self) -> &str {
        "example-fps"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        ArenaGame::view(self, gpu)
    }

    /// A step, and its time added to the next frame's.
    fn advance(&mut self, ms: u32) {
        self.step(ms);
        self.add_frame_time(ms as f32 / 1000.0);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        ArenaGame::frame(self, view)
    }
}
