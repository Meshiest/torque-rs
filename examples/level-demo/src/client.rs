//! The example's client and the server it runs: a client host (script engine, GUI, action maps, TGE's connection half) whose
//! canvas is the menu or the world's view, and while a map is played, the [`WorldServer`] it joined, both on one clock.
//!
//! **The link.** The client's socket is an in-memory one and the server's transport another ([`ClientNet::Memory`]); every
//! step carries the datagrams each sent to the other, so the connection is the engine's own handshake, packets and ghosts,
//! with no socket opened.
//!
//! **A step** ([`LevelDemo::step`]): the engine's client step ([`GameClient::step`]: the client's ticks and as many server
//! ticks as the time covers, the link carried around each); then what the scripts asked of the host: a map picked
//! (`$World::Pick`: the map's server starts and the client joins it) or the world left (`$World::Leave`: the server stops).
//!
//! **A frame** ([`LevelDemo::frame`]): the own figure where the client predicts it, the ghosts as the world view's objects
//! ([`crate::scene`]) and the map's spot lights, the canvas drawn, and the eye: behind the client's own figure (or at its eyes,
//! `V`), kept in front of the cubes and the interiors' walls between. `V` eases the camera from one to the other
//! ([`CameraBlend`], kept in front of them as the camera behind is); the figure is hidden only once the camera has arrived at
//! its eyes.

use std::net::SocketAddrV4;
use std::ops::{Deref, DerefMut};

use torque::app::camera_blend::CameraBlend;
use torque::app::client::{ClientBuilder, ClientNet, ClientSetup, GameClient};
use torque::app::lights::WorldLights;
use torque::app::view::{Daylight, WorldView};
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::{Eye, SceneSource as _};

use crate::files::{GameFiles, MapEntry};
use crate::scene::WorldScene;
use crate::server::{SERVER_ADDR, WorldServer};
use torque::app::ghosts::OwnView;
use torque_example_common::prediction::CubeClient;

/// The client's address on the link.
pub const CLIENT_ADDR: SocketAddrV4 = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50_000);

/// The third-person camera: how far behind the figure, how high above its feet it looks, and how much it looks down.
pub const CAMERA_DISTANCE: f32 = 6.5;
pub const CAMERA_TARGET_HEIGHT: f32 = 1.7;
pub const CAMERA_PITCH: f32 = 0.3;
/// The nearest the camera comes to a cube between it and the figure.
pub const CAMERA_CLEARANCE: f32 = 0.3;

/// The example's client (the engine's [`GameClient`], which it derefs to: its host, its globals, the map's server and their
/// logs) and the level demo's own parts.
pub struct LevelDemo {
    pub client: GameClient<WorldServer>,
    pub files: GameFiles,
    pub maps: Vec<MapEntry>,
    scene: WorldScene,
    /// The map being played (its index in [`LevelDemo::maps`]).
    pub playing: Option<usize>,
    /// The daylight the view was last given (the map's Sky over the default).
    daylight: Daylight,
    /// The camera's ease between first and third person, and the mode it last eased to (first person).
    blend: CameraBlend,
    camera_mode: Option<bool>,
    /// The map's spot lights (SpotLight objects), drawn where their ghosts are.
    lights: WorldLights,
}

impl Deref for LevelDemo {
    type Target = GameClient<WorldServer>;

    fn deref(&self) -> &GameClient<WorldServer> {
        &self.client
    }
}

impl DerefMut for LevelDemo {
    fn deref_mut(&mut self) -> &mut GameClient<WorldServer> {
        &mut self.client
    }
}

impl LevelDemo {
    /// The client over `files`: `client.cs` run, the menu built from the maps found.
    pub fn new(mut files: GameFiles) -> Result<LevelDemo, String> {
        crate::levels::add_levels(&mut files)?;
        let maps = crate::files::maps(&files);
        let mut b = ClientBuilder::new(ClientSetup {
            files: Some(files.mem_files()),
            game_classes: true,
            client_game: Some(std::rc::Rc::new(CubeClient::new(files.mem_files()))),
            net: ClientNet::Memory {
                client: CLIENT_ADDR,
                server: SERVER_ADDR,
            },
            layout_fonts: true,
            ..ClientSetup::default()
        })?;
        b.vm.set_global("$World::MapCount", &maps.len().to_string());
        for (i, m) in maps.iter().enumerate() {
            b.vm.set_global(&format!("$World::MapTitle{i}"), &m.title);
            b.vm.set_global(&format!("$World::MapAbout{i}"), &m.about);
        }
        b.exec("client.cs")?;
        b.invoke("buildMenu")?;
        Ok(LevelDemo {
            client: b.build(),
            files,
            maps,
            scene: WorldScene::default(),
            playing: None,
            daylight: Daylight::default(),
            blend: CameraBlend::default(),
            lights: WorldLights::default(),
            camera_mode: None,
        })
    }

    /// A world view on `gpu`.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    /// Starts map `index`: its server, and the client joins it.
    pub fn start_map(&mut self, index: usize) -> Result<(), String> {
        let map = self.maps.get(index).ok_or("no such map")?.clone();
        self.leave();
        let server = WorldServer::start(&self.files, &map.path)?;
        self.client.start_hosting(server);
        self.playing = Some(index);
        self.call("connectTo", &[SERVER_ADDR.to_string().as_str()])?;
        Ok(())
    }

    /// Stops the server being played, if there is one.
    pub fn leave(&mut self) {
        self.client.stop_hosting();
        self.scene.clear();
        self.playing = None;
        self.blend.reset();
        self.camera_mode = None;
    }

    /// `ms` of the clock (see the module docs).
    pub fn step(&mut self, ms: u32) {
        self.client.step(ms);
        if let Some(pick) = self.take_number("$World::Pick")
            && let Ok(index) = usize::try_from(pick)
            && let Err(e) = self.start_map(index)
        {
            eprintln!("example-level-demo: {e}");
        }
        if self.take_number("$World::Leave").is_some() {
            self.leave();
        }
    }

    /// Whether the canvas shows the world (the server said the map started: `$World::InWorld`).
    pub fn in_world(&self) -> bool {
        self.flag("$World::InWorld")
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

    /// The client's own figure as the last frame drew it.
    pub fn figure(&self) -> Option<OwnView> {
        self.scene.own()
    }

    /// Whether the camera is at the figure's eyes (`V`).
    pub fn first_person(&self) -> bool {
        self.flag("$World::FirstPerson")
    }

    /// Puts the camera there at once, without easing (the shots' second picture).
    pub fn set_first_person(&mut self, on: bool) {
        self.set_global("$World::FirstPerson", if on { "1" } else { "0" });
        self.camera_mode = Some(on);
        self.blend.reset();
    }

    /// The eye this frame: behind the figure or at its eyes; a fixed view of the middle of the map before there is one.
    pub fn eye(&self) -> Eye {
        match self.figure() {
            Some(f) if self.first_person() => {
                let at = f.eye;
                let (s, c) = f.pitch.sin_cos();
                let ahead = [at[0] + f.yaw.sin() * c, at[1] + f.yaw.cos() * c, at[2] - s];
                with_fov(look_at(at, ahead))
            }
            Some(f) => with_fov(self.third_person(&f)),
            None => with_fov(look_at([0.0, -30.0, 18.0], [0.0, 0.0, 0.0])),
        }
    }

    fn third_person(&self, f: &OwnView) -> Eye {
        let target = camera_target(f);
        let pitch = (CAMERA_PITCH + f.pitch * 0.6).clamp(-0.6, 1.2);
        let (s, c) = pitch.sin_cos();
        let back = [-f.yaw.sin() * c, -f.yaw.cos() * c, s];
        let at = self.kept_clear(target, back, CAMERA_DISTANCE, 0.4);
        look_at(at, target)
    }

    /// The point `distance` from `target` along unit `back`, brought in front of the first cube or interior wall between (no
    /// nearer than `nearest`, or the distance when that is nearer).
    fn kept_clear(
        &self,
        target: [f32; 3],
        back: [f32; 3],
        distance: f32,
        nearest: f32,
    ) -> [f32; 3] {
        let reach = match self.scene.sight_hit(target, back, distance) {
            Some(t) => (t - CAMERA_CLEARANCE).max(nearest.min(distance)),
            None => distance,
        };
        [0, 1, 2].map(|i| target[i] + back[i] * reach)
    }

    /// The eye drawn this frame: [`LevelDemo::eye`], eased from the last mode's after a switch (module docs).
    pub fn drawn_eye(&mut self) -> Eye {
        let target = self.eye();
        let figure = self.figure();
        let mut blend = self.blend;
        let eye = blend.eye(target, |p| match figure {
            Some(f) => {
                let from = camera_target(&f);
                let d = [0, 1, 2].map(|i| p[i] - from[i]);
                let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if length < 1e-4 {
                    return p;
                }
                self.kept_clear(from, d.map(|x| x / length), length, 0.0)
            }
            None => p,
        });
        self.blend = blend;
        eye
    }

    /// Whether the camera is easing between modes.
    pub fn camera_blending(&self) -> bool {
        !self.blend.arrived()
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

    /// The frame's motion `dt` seconds after the last, with nothing drawn (a frame's first half): the client's prediction
    /// drawn to the frame, the camera's ease advanced, the figures posed.
    pub fn update_scene(&mut self, dt: f32) {
        self.client.predict_frame();
        let first_person = self.first_person();
        if self
            .camera_mode
            .replace(first_person)
            .is_some_and(|m| m != first_person)
        {
            self.blend.switch();
        }
        self.blend.advance(dt);
        let hidden = first_person && self.blend.arrived();
        {
            let mut st = self.client.host.client.borrow_mut();
            match st.conn.conn.as_mut() {
                Some(conn) => {
                    let conn = &mut **conn;
                    // The camera is the example's own: the figure is seen unless the view is at its eyes.
                    conn.world.hide_control = hidden;
                    self.scene.update(
                        &self.files.kit,
                        &mut conn.world,
                        &conn.predict.ghost_players,
                        dt,
                    );
                }
                None => self.scene.clear(),
            }
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
        let eye = self.drawn_eye();
        let objects = {
            let st = self.client.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.scene.objects(view, &self.files.kit, &conn.world, &eye),
                None => Vec::new(),
            }
        };
        view.set_objects(objects);
        let spots = {
            let st = self.client.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self.lights.spots(&self.files.kit, &conn.world.records),
                None => Vec::new(),
            }
        };
        view.set_spots(spots);
        // The client's camera is the example's own: the GameTSCtrl draws the world whatever the connection's camera is.
        let host = &mut self.client.host;
        host.gui.0.borrow_mut().world_camera = true;
        (host.draw_frame(), eye)
    }
}

impl App for LevelDemo {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "torque-rs level demo".to_string()
    }

    fn log_name(&self) -> &str {
        "example-level-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        LevelDemo::view(self, gpu)
    }

    /// A step, and its time added to the next frame's.
    fn advance(&mut self, ms: u32) {
        self.step(ms);
        self.add_frame_time(ms as f32 / 1000.0);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        LevelDemo::frame(self, view)
    }
}

/// What the camera behind the figure looks at: above its feet.
fn camera_target(f: &OwnView) -> [f32; 3] {
    [
        f.position[0],
        f.position[1],
        f.position[2] + CAMERA_TARGET_HEIGHT,
    ]
}

pub fn with_fov(mut eye: Eye) -> Eye {
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
