//! The client's ghosts as the world view's objects: each StaticShape and Player ghost whose datablock names a shape of the kit,
//! placed where its records put it.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say. Their texture coordinates are made from where each vertex is in
//!   the world (one grid tile a unit, the face's two axes), so a long wall shows square tiles instead of one stretched grid, and
//!   each is tinted by its height: the ground grey-blue, higher cubes warmer. A cube's object is made once and kept until its
//!   record moves it.
//! - **Figures** (Players) stand where their last two records put them, the frame between the two (the server sends one a
//!   tick), facing their yaw. Each plays the kit's idle, walk, run or jump by its speed and its falling flag ([`Gait`]): a change
//!   blends over [`TRANSITION`] seconds, a gait plays at the speed its ground frames carry the figure.

use std::collections::{BTreeMap, HashMap};

use torque::client::conn::tge::TgeWorld;
use torque::ghost_world::world::Pose;
use torque::ghosts::{TgeClass, TgeRecord, Value};
use torque::shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use torque::shapes::shape::Shape;
use torque_example_common::KitAssets;
use torque_example_common::view::{SceneShape, WorldView};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::{Draw, Vertex};

/// Seconds a change of gait blends over.
pub const TRANSITION: f32 = 0.2;
/// Horizontal speeds (units a second) at which a figure walks and runs.
pub const WALK_FROM: f32 = 0.4;
pub const RUN_FROM: f32 = 3.2;
/// How long a server tick is (seconds): the time a figure takes from one record's place to the next.
pub const TICK: f32 = 0.032;
/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;

/// What a figure's thread plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    Idle,
    Walk,
    Run,
    Jump,
}

/// A figure's motion between records and its animation.
struct FigureLook {
    from: [f32; 3],
    to: [f32; 3],
    /// Seconds since `to` came.
    since: f32,
    yaw: f32,
    pitch: f32,
    speed: f32,
    falling: bool,
    instance: ShapeInstance,
    thread: ThreadId,
    detail: usize,
    gait: Gait,
}

/// Where a figure is this frame, for the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureView {
    /// The figure's feet.
    pub position: [f32; 3],
    pub yaw: f32,
    /// Positive looks down.
    pub pitch: f32,
    pub gait: Gait,
}

/// A cube's object, kept with the pose it was made at.
struct KeptCube {
    pose: Pose,
    object: DynamicObject,
    /// Its box in the world (min, max): for the camera's line of sight.
    bounds: ([f32; 3], [f32; 3]),
}

/// The objects of the client's ghosts, frame after frame.
#[derive(Default)]
pub struct WorldScene {
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
    figures: BTreeMap<u32, FigureLook>,
}

/// The id of a ghost's object among the frame's objects.
fn object_id(index: u32) -> u32 {
    index + 1
}

/// The tint of a cube whose top is at `top`: the ground grey-blue, warmer the higher it reaches.
pub fn cube_tint(top: f32) -> [u8; 3] {
    let t = (top / 6.0).clamp(0.0, 1.0);
    let ground = [196.0, 206.0, 222.0];
    let high = [236.0, 178.0, 120.0];
    let low = [218.0, 214.0, 200.0];
    let mix = |a: [f32; 3], b: [f32; 3], k: f32| {
        [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * k).round() as u8)
    };
    if top <= 0.05 {
        mix(ground, ground, 0.0)
    } else {
        let a = mix(low, high, t);
        a.map(|v| v.max(1))
    }
}

/// Texture coordinates from where `v` is: the two world axes across the face its normal points out of, one tile a unit (the
/// grid texture holds four tiles a side).
fn world_uv(v: &Vertex) -> [f32; 2] {
    let [nx, ny, nz] = v.normal.map(f32::abs);
    let [x, y, z] = v.position;
    let (u, w) = if nz >= nx && nz >= ny {
        (x, y)
    } else if nx >= ny {
        (y, z)
    } else {
        (x, z)
    };
    [u / 4.0, -w / 4.0]
}

fn retexture(d: &mut Draw, tint: [u8; 3]) {
    for v in &mut d.vertices {
        v.uv = world_uv(v);
        v.color = [tint[0], tint[1], tint[2], 255];
    }
    if let Some((_, sampler)) = d.texture.as_mut() {
        sampler.repeat = true;
    }
}

impl WorldScene {
    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.cubes.clear();
        self.figures.clear();
    }

    fn shape(&mut self, assets: &KitAssets, path: &[u8]) -> Option<SceneShape> {
        let path = String::from_utf8_lossy(path).replace('\\', "/");
        self.shapes
            .entry(path.clone())
            .or_insert_with(|| SceneShape::load(assets, &path))
            .clone()
    }

    /// The boxes of the cubes, for the camera's line of sight.
    pub fn cube_bounds(&self) -> impl Iterator<Item = ([f32; 3], [f32; 3])> + '_ {
        self.cubes.values().map(|c| c.bounds)
    }

    /// The figure of ghost `index` as this frame draws it.
    pub fn figure(&self, index: u32) -> Option<FigureView> {
        let f = self.figures.get(&index)?;
        Some(FigureView {
            position: f.position(),
            yaw: f.yaw,
            pitch: f.pitch,
            gait: f.gait,
        })
    }

    /// The figures' ghost indices.
    pub fn figure_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.figures.keys().copied()
    }

    /// The frame's objects: `world`'s cubes and figures, the figures moved on by `dt` seconds.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
        dt: f32,
    ) -> Vec<DynamicObject> {
        let mut seen_cubes = Vec::new();
        let mut seen_figures = Vec::new();
        let mut out = Vec::new();
        for (&index, ghost) in &world.records.ghosts {
            let Some(record) = ghost.record.as_ref() else {
                continue;
            };
            let Some(db) = ghost.datablock() else {
                continue;
            };
            let Some(file) = world.datablock_text(db, "shapeFile") else {
                continue;
            };
            match (ghost.class, record) {
                (Some(TgeClass::StaticShape), _) => {
                    let Some(pose) = ghost.pose() else { continue };
                    seen_cubes.push(index);
                    if self.cubes.get(&index).is_some_and(|c| c.pose == pose) {
                        continue;
                    }
                    let Some(shape) = self.shape(assets, file) else {
                        continue;
                    };
                    if let Some(cube) = make_cube(view, assets, &shape, pose, index) {
                        self.cubes.insert(index, cube);
                    }
                }
                (Some(TgeClass::Player), TgeRecord::Player(p)) => {
                    let Some(Pose::Euler { position, yaw, .. }) = ghost.pose() else {
                        continue;
                    };
                    let Some(shape) = self.shape(assets, file) else {
                        continue;
                    };
                    let max_look = match world
                        .datablocks
                        .get(&db)
                        .and_then(|d| d.get("maxLookAngle", 0))
                    {
                        Some(Value::F32(v)) => *v,
                        _ => DEFAULT_MAX_LOOK,
                    };
                    let pitch = torque::ghost_world::tge::head_fractions(record)
                        .map_or(0.0, |h| h[0] * max_look);
                    let falling = p.motion.as_ref().is_some_and(|m| m.falling);
                    seen_figures.push(index);
                    let look = match self.figures.entry(index) {
                        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            let detail = shape.detail().unwrap_or(0);
                            let Some(look) = FigureLook::new(shape.shape(), detail, position)
                            else {
                                continue;
                            };
                            e.insert(look)
                        }
                    };
                    look.update(shape.shape(), position, yaw, pitch, falling, dt);
                    let transform = look.transform();
                    out.push(view.shape_object(
                        assets,
                        &shape,
                        &look.instance,
                        &transform,
                        object_id(index),
                    ));
                }
                _ => {}
            }
        }
        self.cubes.retain(|i, _| seen_cubes.contains(i));
        self.figures.retain(|i, _| seen_figures.contains(i));
        out.extend(self.cubes.values().map(|c| c.object.clone()));
        out
    }
}

fn make_cube(
    view: &mut WorldView,
    assets: &KitAssets,
    shape: &SceneShape,
    pose: Pose,
    index: u32,
) -> Option<KeptCube> {
    let (m, scale) = pose_matrix(&pose, None)?;
    let instance = shape.instance()?;
    // The box around the turned cube's eight corners.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for corner in 0..8 {
        let local = [0, 1, 2].map(|i| {
            let sign = if corner & (1 << i) == 0 { -0.5 } else { 0.5 };
            sign * scale[i]
        });
        let p = m.mul_p(local);
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let bounds = (lo, hi);
    let tint = cube_tint(bounds.1[2]);
    let object = view.shape_object_with(
        assets,
        shape,
        &instance,
        &m,
        scale,
        object_id(index),
        &mut |d| retexture(d, tint),
    );
    Some(KeptCube {
        pose,
        object,
        bounds,
    })
}

impl FigureLook {
    fn new(shape: &Shape, detail: usize, at: [f32; 3]) -> Option<FigureLook> {
        let mut instance = ShapeInstance::new(shape, &mut |_| true).ok()?;
        let thread = instance.create_thread(shape)?;
        let idle = find_sequence(shape, b"idle")?;
        instance.set_sequence(shape, thread, idle as i32, 0.0);
        Some(FigureLook {
            from: at,
            to: at,
            since: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            speed: 0.0,
            falling: false,
            instance,
            thread,
            detail,
            gait: Gait::Idle,
        })
    }

    fn position(&self) -> [f32; 3] {
        let k = (self.since / TICK).clamp(0.0, 1.0);
        [0, 1, 2].map(|i| self.from[i] + (self.to[i] - self.from[i]) * k)
    }

    fn transform(&self) -> Mat {
        let mut m = Mat::euler_z(self.yaw);
        m.set_column3(self.position());
        m
    }

    fn update(
        &mut self,
        shape: &Shape,
        at: [f32; 3],
        yaw: f32,
        pitch: f32,
        falling: bool,
        dt: f32,
    ) {
        if at != self.to {
            let moved = ((at[0] - self.to[0]).powi(2) + (at[1] - self.to[1]).powi(2)).sqrt();
            let step = self.since.max(TICK);
            // A jump of more than a few units is a respawn: no motion between.
            if moved > 4.0 {
                self.from = at;
                self.speed = 0.0;
            } else {
                self.from = self.position();
                let measured = moved / step;
                self.speed += (measured - self.speed) * 0.5;
            }
            self.to = at;
            self.since = 0.0;
        } else {
            self.since += dt;
            if self.since > TICK * 3.0 {
                self.speed *= 0.5;
            }
        }
        self.yaw = yaw;
        self.pitch = pitch;
        self.falling = falling;
        let gait = if falling {
            Gait::Jump
        } else if self.speed >= RUN_FROM {
            Gait::Run
        } else if self.speed >= WALK_FROM {
            Gait::Walk
        } else {
            Gait::Idle
        };
        if gait != self.gait {
            let name: &[u8] = match gait {
                Gait::Idle => b"idle",
                Gait::Walk => b"walk",
                Gait::Run => b"run",
                Gait::Jump => b"jump",
            };
            if let Some(seq) = find_sequence(shape, name) {
                self.instance
                    .transition_to_sequence(shape, self.thread, seq, 0.0, TRANSITION);
            }
            self.gait = gait;
        }
        let ground = match gait {
            Gait::Walk => torque_example_assets::shapes::WALK_SPEED,
            Gait::Run => torque_example_assets::shapes::RUN_SPEED,
            _ => 0.0,
        };
        let scale = if ground > 0.0 {
            (self.speed / ground).clamp(0.3, 2.5)
        } else {
            1.0
        };
        if let Some(t) = self.instance.thread_mut(self.thread) {
            t.time_scale = scale;
        }
        self.instance.advance_time(shape, self.thread, dt);
        // The kit's figure: nothing to run out of.
        let _ = self
            .instance
            .animate(shape, self.detail, None, &mut |_| true);
    }
}
