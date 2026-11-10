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
//! `Tab`), both looking where the figure looks ([`torque::app::camera_behind`]: `Tab` moves the camera, never turns it). The camera behind rides a
//! boom ([`torque::app::camera_boom::CameraBoom`]) back from above the figure's feet (no higher than its eye), pulled in
//! fast in front of the cubes, the interiors' walls, the terrain and the water that come between and let out slowly when they
//! go; it looks through glass. `Tab` (`setCameraMode`: the camera modes, [`torque::app::camera_modes`]) moves the camera from one to the other ([`torque::app::camera_blend::CameraBlend`], kept in front of them as the camera behind is); the figure is hidden while the
//! camera is close to its eyes ([`CLOSE`]), on the way in and on the way out. A new figure (a respawn) or a teleport cuts the
//! camera to it ([`OwnView::cut`]), and while the next figure is on its way the view stays where it was.
//!
//! **The ground.** A map with a terrain or water ([`MapGround`]: the valley) has them drawn as the map starts, the terrain also
//! the own figure's ground in the prediction ([`CubeClient::set_terrain`]) and the camera kept above it; its day (a
//! `TimeOfDay` under a `ScatterSky`) lights the view and the sky's clouds are sunlit cumulus ([`WorldLights`]).

use std::net::SocketAddrV4;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use torque::app::camera_blend::CLOSE;
use torque::app::camera_modes::{
    CameraCx, CameraModes, CameraWorld, FIRST_PERSON, Rig, THIRD_PERSON,
};
use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::lights::WorldLights;
use torque::app::view::{Daylight, WorldView};
use torque::app::{App, Host};
use torque::engine::Engine;
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::hd::clouds::CloudLook;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::{Eye, SceneSource as _};

use crate::files::{GameFiles, MapEntry};
use crate::ground::MapGround;
use crate::plugin::{Level, level_engine};
use crate::scene::WorldScene;
use crate::server::{SERVER_ADDR, WorldServer};
use torque::app::ghosts::OwnView;
use torque_example_common::prediction::CubeClient;

/// The client's address on the link.
pub const CLIENT_ADDR: SocketAddrV4 = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50_000);

/// The third-person camera: how far behind the figure, how high above its feet its boom starts, and how far up the view it
/// rides off the boom's line (the figure a little below the middle of the view).
pub const CAMERA_DISTANCE: f32 = 6.5;
pub const CAMERA_TARGET_HEIGHT: f32 = 1.7;
pub const CAMERA_LIFT: f32 = 0.6;
/// Where a map's own cloud clock starts over (seconds): long past any session, short enough to keep its precision.
pub const CLOUD_CLOCK_WRAP: f32 = 100_000.0;
/// The nearest the camera comes to a cube between it and the figure, and to the boom's pivot behind the figure.
pub const CAMERA_CLEARANCE: f32 = 0.3;
pub const CAMERA_NEAREST: f32 = 0.4;

/// The camera behind the figure: the numbers above as the built-in third person and first person take them.
pub fn camera_rig() -> Rig {
    Rig {
        distance: CAMERA_DISTANCE,
        target_height: CAMERA_TARGET_HEIGHT,
        lift: CAMERA_LIFT,
        nearest: CAMERA_NEAREST,
        clearance: CAMERA_CLEARANCE,
    }
}

/// The example's client (the engine's [`GameClient`], which it derefs to: its host, its globals, the map's server and their
/// logs) and the level demo's own parts.
pub struct LevelDemo {
    pub client: GameClient<WorldServer>,
    pub files: Rc<GameFiles>,
    /// Files only the map's server has (the mods it requires of its clients).
    server_mods: Vec<(String, Vec<u8>)>,
    engine: Engine,
    pub maps: Vec<MapEntry>,
    scene: WorldScene,
    /// The map being played (its index in [`LevelDemo::maps`]).
    pub playing: Option<usize>,
    /// The daylight the view was last given (the map's Sky over the default).
    daylight: Daylight,
    /// The camera modes (first person and the camera behind the figure on its boom), the one in use and the ease between them.
    modes: Rc<CameraModes>,
    /// Where the camera was drawn last frame.
    drawn_at: Option<[f32; 3]>,
    /// The eye last drawn from the figure, held while the server's next figure is on its way (a respawn).
    held: Option<Eye>,
    /// The map's spot lights (SpotLight objects), drawn where their ghosts are, and its day.
    lights: WorldLights,
    /// The prediction's answers (the terrain the own figure meets).
    cube: Rc<CubeClient>,
    /// The map's terrain and water, and whether the view has yet to be given them.
    pub ground: MapGround,
    ground_changed: bool,
    /// The clouds' own clock of a map that runs them faster ([`MapGround::cloud_time_scale`]).
    cloud_clock: f32,
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
    /// The client over `files`: [`level_engine`] built, then [`LevelDemo::from_engine`].
    pub fn new(files: GameFiles) -> Result<LevelDemo, String> {
        let engine = level_engine(files)?.build().map_err(|e| e.to_string())?;
        LevelDemo::from_engine(&engine)
    }

    /// The client `engine` makes (over the files its [`LevelPlugin`](crate::plugin::LevelPlugin) holds): `client.cs` run, the menu built from the maps
    /// found.
    pub fn from_engine(engine: &Engine) -> Result<LevelDemo, String> {
        // The shared level, not the resource's borrow, which would be held across the client's recipes.
        let level = Rc::clone(
            &*engine
                .resource::<Rc<Level>>()
                .ok_or("the engine has no level demo (LevelPlugin)")?,
        );
        let mut b = ClientBuilder::from_engine(
            engine,
            Some(level.files.mem_files()),
            ClientNet::Memory {
                client: CLIENT_ADDR,
                server: SERVER_ADDR,
            },
        )?;
        b.exec("client.cs")?;
        b.invoke("buildMenu")?;
        let client = b.build();
        let modes = client
            .camera_modes()
            .ok_or("the engine has no camera modes (CameraModesPlugin)")?;
        // The camera behind the figure is the one in use until `Tab`.
        modes.set_now(THIRD_PERSON);
        Ok(LevelDemo {
            client,
            engine: engine.clone(),
            files: Rc::clone(&level.files),
            server_mods: level.server_mods.clone(),
            maps: level.maps.clone(),
            scene: WorldScene::default(),
            playing: None,
            daylight: Daylight::default(),
            modes,
            lights: WorldLights::default(),
            drawn_at: None,
            held: None,
            cube: Rc::clone(&level.cube),
            ground: MapGround::default(),
            ground_changed: false,
            cloud_clock: 0.0,
        })
    }

    /// The files only a server of this game has (the mods it requires of its clients).
    pub fn server_mods(&self) -> &[(String, Vec<u8>)] {
        &self.server_mods
    }

    /// The engine this client and its servers are made by.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// A world view on `gpu`, the sky's clouds sunlit cumulus (a map with a day has them), the ground of the map being played.
    pub fn view(&self, gpu: &Gpu) -> WorldView {
        let mut view = WorldView::new(gpu);
        view.set_cloud_look(Some(CloudLook::SUNLIT_CUMULUS));
        self.ground.apply(&mut view, &self.files.kit);
        view
    }

    /// Starts map `index`: its server, and the client joins it.
    pub fn start_map(&mut self, index: usize) -> Result<(), String> {
        let map = self.prepare_map(index)?;
        let server =
            WorldServer::start_with(&self.engine, &self.files, &self.server_mods, &map.path)?;
        self.client.start_hosting(server);
        self.enter_map(index, &map)
    }

    /// Joins map `index` as served by a server another part of the process runs (at [`SERVER_ADDR`], started over a copy of this
    /// client's files with [`WorldServer::start_with`]): the client plays it as it does a map of its own server.
    pub fn join_map(&mut self, index: usize) -> Result<(), String> {
        let map = self.prepare_map(index)?;
        self.enter_map(index, &map)
    }

    /// The client left any map it was in, and has the ground map `index` has.
    fn prepare_map(&mut self, index: usize) -> Result<MapEntry, String> {
        let map = self.maps.get(index).ok_or("no such map")?.clone();
        self.leave();
        let text =
            String::from_utf8_lossy(self.files.file(&map.path).unwrap_or_default()).into_owned();
        self.ground = MapGround::read(&self.files, &text)?;
        self.cube.set_terrain(self.ground.terrain.as_ref());
        self.cube.set_water(&self.ground.waters);
        self.scene.set_water(&self.ground.waters);
        self.ground_changed = true;
        Ok(map)
    }

    // The map's name hangs the plaza's rope (the `rope` feature).
    fn enter_map(
        &mut self,
        index: usize,
        #[cfg_attr(not(feature = "rope"), allow(unused_variables))] map: &MapEntry,
    ) -> Result<(), String> {
        self.playing = Some(index);
        self.call("connectTo", &[SERVER_ADDR.to_string().as_str()])?;
        #[cfg(feature = "rope")]
        crate::ropes::hang(self, &crate::shots::map_name(&map.path))?;
        Ok(())
    }

    /// Stops the server being played, if there is one.
    pub fn leave(&mut self) {
        #[cfg(feature = "rope")]
        crate::ropes::take_down(self);
        self.client.stop_hosting();
        self.scene.clear();
        self.playing = None;
        self.modes.cut();
        self.held = None;
        self.lights.clear();
        if !self.ground.is_empty() {
            self.ground = MapGround::default();
            self.cube.set_terrain(None);
            self.cube.set_water(&[]);
            self.scene.set_water(&[]);
            self.ground_changed = true;
        }
    }

    /// `ms` of the clock (see the module docs).
    pub fn step(&mut self, ms: u32) {
        self.client.step(ms);
        // The scripts swim with the keys while the figure swims as the client predicts it (`$World::Swimming`).
        let swimming = self.swimming();
        if swimming != self.flag("$World::Swimming") {
            self.set_global("$World::Swimming", if swimming { "1" } else { "0" });
            if let Err(e) = self.call("updateMove", &[]) {
                eprintln!("example-level-demo: {e}");
            }
        }
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

    /// Whether the client's own figure swims, as the client predicts it.
    pub fn swimming(&self) -> bool {
        self.with_world(|w| w.control_state.as_ref().map(|c| c.pose))
            .flatten()
            == Some(torque::sim::player::Pose::Swim)
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

    /// The props the last frame drew as still geometry (they sleep): the moving shapes it drew leave them out.
    pub fn resting_props(&self) -> usize {
        self.scene.resting_count()
    }

    /// Whether the camera is at the figure's eyes (`Tab`).
    pub fn first_person(&self) -> bool {
        self.modes.is_active(FIRST_PERSON)
    }

    /// Puts the camera there at once, without easing (the shots' second picture).
    pub fn set_first_person(&mut self, on: bool) {
        self.modes
            .set_now(if on { FIRST_PERSON } else { THIRD_PERSON });
    }

    /// Runs `f` on the camera modes with this frame's context: the figure as the last frame placed it, `dt` seconds on.
    fn with_camera<R>(&self, dt: f32, f: impl FnOnce(&CameraModes, &CameraCx<'_>) -> R) -> R {
        let figure = self.figure();
        f(
            &self.modes,
            &CameraCx::new(self, dt).with_control(figure.as_ref()),
        )
    }

    /// The eye this frame: behind the figure or at its eyes; a fixed view of the middle of the map before there is one.
    pub fn eye(&self) -> Eye {
        let fixed = with_fov(look_at([0.0, -30.0, 18.0], [0.0, 0.0, 0.0]));
        self.with_camera(0.0, |modes, cx| modes.target(cx, fixed))
    }

    /// The eye drawn this frame: [`LevelDemo::eye`], its camera eased from the last mode's after a switch, its look the
    /// mode's own (module docs).
    pub fn drawn_eye(&mut self) -> Eye {
        let figure = self.figure();
        // The figure gone and the server's next one not here yet (a respawn): the view stays where it was.
        if self.playing.is_some()
            && figure.is_none()
            && let Some(eye) = self.held
        {
            return eye;
        }
        let fixed = with_fov(look_at([0.0, -30.0, 18.0], [0.0, 0.0, 0.0]));
        let eye = self.with_camera(0.0, |modes, cx| modes.drawn(cx, fixed));
        self.drawn_at = Some(eye.position);
        self.held = figure.map(|_| eye);
        eye
    }

    /// The share of the way from `from` to `to` at which the camera's line of sight first meets something, if it does
    /// ([`WorldScene::sight_share`]).
    pub fn sight_share(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        self.scene.sight_share(from, to)
    }

    /// Whether the camera is easing between modes.
    pub fn camera_blending(&self) -> bool {
        self.modes.blending()
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
        self.with_camera(dt, |modes, cx| modes.begin(cx));
        // The figure is hidden while the camera is close to its eyes, on the way in and on the way out.
        let hidden = match (self.figure(), self.drawn_at) {
            (Some(f), Some(p)) => near_eyes(p, f.eye),
            _ => first_person,
        };
        {
            let mut st = self.client.host.client.borrow_mut();
            match st.conn.conn.as_mut() {
                Some(conn) => {
                    let conn = &mut **conn;
                    // The camera is the example's own: the figure is seen unless the view is at its eyes.
                    conn.world.hide_control = hidden;
                    // Its arms still drawn there.
                    torque_example_common::show_first_person_arms(&mut conn.world);
                    self.scene.update(
                        &self.files.kit,
                        &mut conn.world,
                        &conn.predict.ghost_players,
                        dt,
                    );
                    self.lights.update(&conn.world.records, dt);
                }
                None => self.scene.clear(),
            }
        }
        // A new figure (a respawn) or a teleport cuts the camera to it: nothing eased across the way between.
        if self.figure().is_some_and(|f| f.cut) {
            self.modes.cut();
        }
        self.with_camera(dt, |modes, cx| modes.update(cx));
    }

    /// The frame: the ghosts into `view`, the canvas drawn (with the world's rectangle) and the eye.
    pub fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        if std::mem::take(&mut self.ground_changed) {
            self.ground.apply(view, &self.files.kit);
            // The last map's day is forgotten: the next is lit by its own (or by its Sky's daylight alone).
            view.clear_day();
            view.set_daylight(&self.daylight);
        }
        if let Some(d) = self.sky_daylight()
            && d != self.daylight
        {
            view.set_daylight(&d);
            self.daylight = d;
        }
        let dt = self.client.take_frame_time();
        // The water's waves and the sky's layers run with the game's clock (the image's grain stays still).
        view.advance_time(dt);
        let scale = self.ground.cloud_time_scale;
        if scale > 0.0 {
            self.cloud_clock = advance_cloud_clock(self.cloud_clock, dt, scale);
            view.set_cloud_time(Some(self.cloud_clock));
        } else {
            view.set_cloud_time(None);
        }
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
        {
            let st = self.client.host.client.borrow();
            match st.conn.conn.as_ref() {
                Some(conn) => self
                    .lights
                    .apply(view, &self.files.kit, &conn.world.records),
                None => view.set_spots(Vec::new()),
            }
        }
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

pub fn with_fov(mut eye: Eye) -> Eye {
    eye.fov = None;
    eye
}

pub use torque_example_common::ray_box;

/// Whether the camera drawn at `camera` is within [`CLOSE`] of a figure's `eyes`: the figure is hidden.
fn near_eyes(camera: [f32; 3], eyes: [f32; 3]) -> bool {
    (0..3).map(|k| (camera[k] - eyes[k]).powi(2)).sum::<f32>() < CLOSE * CLOSE
}

/// The clouds' clock `clock` (seconds) run on `dt` seconds at `scale` times the clock's pace, starting over at
/// [`CLOUD_CLOCK_WRAP`], and from 0 when it is no longer a finite number.
pub fn advance_cloud_clock(clock: f32, dt: f32, scale: f32) -> f32 {
    let next = (clock + dt * scale) % CLOUD_CLOCK_WRAP;
    if next.is_finite() { next } else { 0.0 }
}

/// What the camera keeps clear of: the cubes and interiors the scene holds, and the terrain and water.
impl CameraWorld for LevelDemo {
    fn clear_share(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        match (
            self.scene.sight_share(from, to),
            self.ground.line_share(from, to),
        ) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    }

    fn clear_hit(&self, from: [f32; 3], dir: [f32; 3], reach: f32) -> Option<f32> {
        let end = [0, 1, 2].map(|i| from[i] + dir[i] * reach);
        let ground = self.ground.line_share(from, end).map(|s| s * reach);
        match (self.scene.sight_hit(from, dir, reach), ground) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    }

    fn ground(&self, x: f32, y: f32) -> Option<f32> {
        self.ground.surface(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cloud_clock_runs_wraps_and_starts_over_when_not_finite() {
        assert_eq!(advance_cloud_clock(10.0, 0.5, 150.0), 85.0);
        assert!(advance_cloud_clock(CLOUD_CLOCK_WRAP - 1.0, 1.0, 2.0) < 2.0);
        assert_eq!(advance_cloud_clock(f32::NAN, 0.5, 150.0), 0.0);
        assert_eq!(advance_cloud_clock(0.0, f32::INFINITY, 150.0), 0.0);
        assert_eq!(advance_cloud_clock(0.0, 0.5, f32::NAN), 0.0);
    }
}
