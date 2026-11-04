//! The client's ghosts as the world view's objects, and the eye.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say, their texture coordinates made from where each vertex is (one
//!   grid tile a unit) and tinted by height. A cube's object is kept until its record moves it.
//! - **Figures** (Players) stand where their last two records put them, the frame between the two, facing their yaw, animated
//!   as TGE's client animates a Player ([`torque::client::player_anim`]). A figure that holds an image (its record's first
//!   image slot) plays the kit's `hold` on its arm thread, and the image's shape is drawn in its hand: the figure's `mount0`
//!   node times the inverse of the image's `mountPoint` (`ShapeBase::getRenderImageTransform`). A new fire count plays the
//!   image's `fire`.
//! - **The own figure** (the ghost the server named, [`OwnLook`]) is not drawn: the eye is at its `eye` node, turned as the
//!   client's mouse turned it (the moves carry the same turns to the server, so the view does not wait for them), and the
//!   pistol is drawn at the eye times the image's `eyeOffset` (a first-person image).
//! - **Bolts** (Projectiles) fly on from their first record along its velocity (the client runs them on its own, as TGE's
//!   client runs a projectile ghost) and are drawn along it, turning slowly about their flight, until their explosion record
//!   hides them. A bolt whose datablock names a `particleEmitter` leaves a trail of its particles behind it
//!   ([`Emitter::emit_trail`] over each frame's flight, as `Projectile::processTick` emits on the client); a gone bolt's trail
//!   is left to fade out (`deleteWhenEmpty`).
//!
//! Credit: the image's hand transform follows `ShapeBase::getRenderImageTransform` in Torque3D's MIT-licensed
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! example's own.

use std::collections::{BTreeMap, HashMap};

use torque::client::conn::tge::TgeWorld;
use torque::client::player_anim::{PlayerAnimation, PlayerMotion};
use torque::ghost_world::world::Pose;
use torque::ghosts::{TgeClass, TgeRecord, Value};
use torque::shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use torque_example_common::KitAssets;
use torque_example_common::view::{SceneShape, WorldView};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::particles::{Emitter, Rng};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Vertex};

/// How long a server tick is (seconds): the time a figure takes from one record's place to the next.
pub const TICK: f32 = 0.032;
/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;
/// The eye's height above the feet when the shape has no `eye` node.
pub const EYE_HEIGHT: f32 = 1.75;
/// The field of view (degrees, horizontal).
pub const FOV: u8 = 90;
/// How fast a bolt turns about its flight (radians a second).
pub const BOLT_SPIN: f32 = std::f32::consts::TAU;

/// The id of a ghost's object among the frame's objects (the first-person pistol is 1).
fn object_id(index: u32, part: u32) -> u32 {
    2 + index * 4 + part
}

/// The tint of a cube whose top is at `top`: the floor a warm sand, the walls and blocks a cool slate, lighter the higher.
pub fn cube_tint(top: f32) -> [u8; 3] {
    if top <= 0.05 {
        return [214, 198, 170];
    }
    let t = (top / 5.0).clamp(0.0, 1.0);
    let low = [132.0, 146.0, 168.0];
    let high = [196.0, 206.0, 224.0];
    [0, 1, 2].map(|i| (low[i] + (high[i] - low[i]) * t).round() as u8)
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

/// A cube's object, kept with the pose it was made at.
struct KeptCube {
    pose: Pose,
    object: DynamicObject,
}

/// A held image: its shape, its instance, its `fire` thread and the fire count last seen.
struct Held {
    shape: SceneShape,
    instance: ShapeInstance,
    fire: Option<ThreadId>,
    fire_count: Option<u32>,
    /// The image's `mountPoint` node at rest, inverted.
    mount_inverse: Mat,
    /// The datablock's `eyeOffset`.
    eye_offset: [f32; 3],
}

impl Held {
    fn new(shape: SceneShape, eye_offset: [f32; 3]) -> Option<Held> {
        let mut instance = shape.instance()?;
        let fire = find_sequence(shape.shape(), b"fire").and_then(|seq| {
            let t = instance.create_thread(shape.shape())?;
            instance.set_sequence(shape.shape(), t, seq as i32, 1.0);
            Some(t)
        });
        let detail = shape.detail().unwrap_or(0);
        let _ = instance.animate(shape.shape(), detail, None, &mut |_| true);
        let mount_inverse = shape
            .node("mountPoint")
            .and_then(|n| instance.node_transform(n))
            .map_or(Mat::IDENTITY, |m| Mat { m: *m }.affine_inverse());
        Some(Held {
            shape,
            instance,
            fire,
            fire_count: None,
            mount_inverse,
            eye_offset,
        })
    }

    /// The frame: a new fire count restarts `fire`, then `dt` of it.
    fn update(&mut self, fire_count: u32, dt: f32) {
        let shape = self.shape.shape();
        if let Some(t) = self.fire {
            if self.fire_count.is_some_and(|c| c != fire_count)
                && let Some(seq) = find_sequence(shape, b"fire")
            {
                self.instance.set_sequence(shape, t, seq as i32, 0.0);
            }
            self.instance.advance_time(shape, t, dt);
        }
        self.fire_count = Some(fire_count);
        let detail = self.shape.detail().unwrap_or(0);
        let _ = self.instance.animate(shape, detail, None, &mut |_| true);
    }
}

/// A figure's motion between records and its animation.
struct Figure {
    shape: SceneShape,
    from: [f32; 3],
    to: [f32; 3],
    /// Seconds since `to` came.
    since: f32,
    yaw: f32,
    pitch: f32,
    instance: ShapeInstance,
    anim: PlayerAnimation,
    held: Option<Held>,
    held_db: Option<u32>,
}

impl Figure {
    fn position(&self) -> [f32; 3] {
        let k = (self.since / TICK).clamp(0.0, 1.0);
        [0, 1, 2].map(|i| self.from[i] + (self.to[i] - self.from[i]) * k)
    }

    fn transform(&self) -> Mat {
        let mut m = Mat::euler_z(self.yaw);
        m.set_column3(self.position());
        m
    }

    /// The eye: the `eye` node's place on the figure, turned by the yaw and the head's pitch.
    fn eye_transform(&self) -> Mat {
        let body = self.transform();
        let at = self
            .shape
            .node("eye")
            .and_then(|n| self.instance.node_transform(n))
            .map_or([0.0, 0.0, EYE_HEIGHT], |m| [m[3], m[7], m[11]]);
        let mut eye = Mat::mul(&Mat::euler_z(self.yaw), &Mat::euler_x(self.pitch));
        eye.set_column3(body.mul_p(at));
        eye
    }
}

/// A bolt: where its first record put it and when, and its trail.
struct Bolt {
    start: [f32; 3],
    velocity: [f32; 3],
    age: f32,
    trail: Option<Emitter>,
}

impl Bolt {
    fn position(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| self.start[i] + self.velocity[i] * self.age)
    }
}

/// A new trail for a bolt of ProjectileData `db`: its `particleEmitter`, when the client holds it as a `ParticleEmitterData`
/// with its particles.
pub fn bolt_trail(world: &TgeWorld, db: u32) -> Option<Emitter> {
    let id = match world.datablocks.get(&db)?.get("particleEmitter", 0)? {
        Value::Id(id) => (*id)?,
        Value::Int(v) => u32::try_from(*v).ok()?,
        _ => return None,
    };
    Emitter::new(&world.records, id)
}

/// Which ghost is the client's own figure, and where the client's mouse has turned its view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnLook {
    pub ghost: u32,
    pub yaw: f32,
    /// Positive looks down.
    pub pitch: f32,
}

/// Where the own figure is this frame, for the camera and the tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnView {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub eye: [f32; 3],
}

/// The objects of the client's ghosts, frame after frame.
#[derive(Default)]
pub struct ArenaScene {
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
    figures: BTreeMap<u32, Figure>,
    bolts: BTreeMap<u32, Bolt>,
    /// The trails of the bolts gone, fading out.
    fading: Vec<Emitter>,
    rng: Option<Rng>,
    /// The live particles of every trail.
    live: usize,
    /// The frame time the trails have not run yet (ms, under one).
    trail_ms: f32,
    own: Option<OwnView>,
    /// The bolts drawn in the last frame, and the other figures holding an image.
    pub bolts_drawn: usize,
    pub holders_drawn: usize,
    /// The trail particles drawn in the last frame.
    pub trail_particles: usize,
}

fn record_images(record: &TgeRecord) -> Option<(u32, u32)> {
    let sb = torque::ghost_world::tge::shape_part(record)?;
    let image = sb.body.as_ref()?.images.as_ref()?[0].as_ref()?;
    Some((image.datablock?, image.fire_count))
}

/// The translation of a datablock's transform field (an image's `eyeOffset`: the identity is not sent, so none is zero).
fn offset_of(world: &TgeWorld, db: u32, field: &str) -> [f32; 3] {
    match world.datablocks.get(&db).and_then(|d| d.get(field, 0)) {
        Some(Value::Xform(Some(x))) => [x[0], x[1], x[2]].map(f32::from_bits),
        _ => [0.0; 3],
    }
}

impl ArenaScene {
    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.cubes.clear();
        self.figures.clear();
        self.bolts.clear();
        self.fading.clear();
        self.live = 0;
        self.own = None;
    }

    /// Where the bolts were drawn in the last frame.
    pub fn bolt_positions(&self) -> Vec<[f32; 3]> {
        self.bolts.values().map(Bolt::position).collect()
    }

    fn shape(&mut self, assets: &KitAssets, path: &[u8]) -> Option<SceneShape> {
        let path = String::from_utf8_lossy(path).replace('\\', "/");
        self.shapes
            .entry(path.clone())
            .or_insert_with(|| SceneShape::load(assets, &path))
            .clone()
    }

    /// The own figure as the last frame placed it.
    pub fn own(&self) -> Option<OwnView> {
        self.own
    }

    /// The other figures' positions (feet) as the last frame drew them.
    pub fn others(&self) -> Vec<[f32; 3]> {
        self.figures.values().map(Figure::position).collect()
    }

    /// The frame's objects: `world`'s cubes, figures and bolts, moved on by `dt` seconds.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
        own: Option<OwnLook>,
        dt: f32,
    ) -> Vec<DynamicObject> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        self.own = None;
        self.bolts_drawn = 0;
        self.holders_drawn = 0;
        // The trails run whole milliseconds; the rest waits for the next frame.
        self.trail_ms += dt * 1000.0;
        let ms = self.trail_ms.floor().clamp(0.0, 1000.0) as u32;
        self.trail_ms = (self.trail_ms - ms as f32).clamp(0.0, 1.0);
        let mut rng = self.rng.take().unwrap_or_else(|| Rng::new(0x5eed));
        for e in &mut self.fading {
            e.advance(ms, &mut self.live);
        }
        self.fading.retain(|e| !e.particles.is_empty());
        for (&index, ghost) in &world.records.ghosts {
            let Some(record) = ghost.record.as_ref() else {
                continue;
            };
            let Some(db) = ghost.datablock() else {
                continue;
            };
            match (ghost.class, record) {
                (Some(TgeClass::StaticShape), _) => {
                    let Some(file) = world.datablock_text(db, "shapeFile") else {
                        continue;
                    };
                    let Some(pose) = ghost.pose() else { continue };
                    seen.push(index);
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
                (Some(TgeClass::Player), TgeRecord::Player(_)) => {
                    let Some(file) = world.datablock_text(db, "shapeFile") else {
                        continue;
                    };
                    let Some(Pose::Euler { position, yaw, .. }) = ghost.pose() else {
                        continue;
                    };
                    let Some(shape) = self.shape(assets, file) else {
                        continue;
                    };
                    seen.push(index);
                    let max_look = match world
                        .datablocks
                        .get(&db)
                        .and_then(|d| d.get("maxLookAngle", 0))
                    {
                        Some(Value::F32(v)) => *v,
                        _ => DEFAULT_MAX_LOOK,
                    };
                    let look = own.filter(|o| o.ghost == index);
                    let is_own = look.is_some();
                    let (yaw, pitch) = match look {
                        Some(o) => (o.yaw, o.pitch),
                        None => {
                            let head = torque::ghost_world::tge::head_fractions(record)
                                .map_or(0.0, |h| h[0]);
                            (yaw, head * max_look)
                        }
                    };
                    let image = record_images(record);
                    let held_shape = image.and_then(|(img, _)| {
                        let file = world.datablock_text(img, "shapeFile")?.to_vec();
                        let eye_offset = offset_of(world, img, "eyeOffset");
                        Some((self.shape(assets, &file)?, eye_offset))
                    });
                    let figure = match self.figures.entry(index) {
                        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            let Some(mut instance) = shape.instance() else {
                                continue;
                            };
                            let Some(anim) = PlayerAnimation::new(shape.shape(), &mut instance)
                            else {
                                continue;
                            };
                            e.insert(Figure {
                                shape: shape.clone(),
                                from: position,
                                to: position,
                                since: 0.0,
                                yaw,
                                pitch,
                                instance,
                                anim,
                                held: None,
                                held_db: None,
                            })
                        }
                    };
                    figure.move_to(position, dt);
                    figure.yaw = yaw;
                    figure.pitch = pitch;
                    // The held image.
                    if image.map(|i| i.0) != figure.held_db {
                        figure.held_db = image.map(|i| i.0);
                        figure.held =
                            held_shape.and_then(|(shape, offset)| Held::new(shape, offset));
                    }
                    let hold = figure
                        .held
                        .as_ref()
                        .and_then(|_| figure.anim.actions().iter().position(|a| a.name == b"hold"));
                    let mut motion = PlayerMotion::of_record(record, yaw).unwrap_or(PlayerMotion {
                        yaw,
                        ..PlayerMotion::default()
                    });
                    motion.arm_action = hold.and_then(|h| u32::try_from(h).ok());
                    motion.look = ((pitch / max_look + 1.0) * 0.5).clamp(0.0, 1.0);
                    let s = figure.shape.shape();
                    figure.anim.update(s, &mut figure.instance, &motion, dt);
                    let detail = figure.shape.detail().unwrap_or(0);
                    let _ = figure.instance.animate(s, detail, None, &mut |_| true);
                    if let (Some(held), Some((_, fire_count))) = (figure.held.as_mut(), image) {
                        held.update(fire_count, dt);
                    }
                    let body = figure.transform();
                    if is_own {
                        let eye = figure.eye_transform();
                        self.own = Some(OwnView {
                            position: figure.position(),
                            yaw,
                            pitch,
                            eye: eye.column(3),
                        });
                        if let Some(held) = figure.held.as_ref() {
                            let mut offset = Mat::IDENTITY;
                            offset.set_column3(held.eye_offset);
                            let m = Mat::mul(&eye, &offset);
                            out.push(view.shape_object(assets, &held.shape, &held.instance, &m, 1));
                        }
                        continue;
                    }
                    out.push(view.shape_object(
                        assets,
                        &figure.shape,
                        &figure.instance,
                        &body,
                        object_id(index, 0),
                    ));
                    if let (Some(held), Some(hand)) = (
                        figure.held.as_ref(),
                        figure
                            .shape
                            .node("mount0")
                            .and_then(|n| figure.instance.node_transform(n)),
                    ) {
                        let m = Mat::mul(&Mat::mul(&body, &Mat { m: *hand }), &held.mount_inverse);
                        out.push(view.shape_object(
                            assets,
                            &held.shape,
                            &held.instance,
                            &m,
                            object_id(index, 1),
                        ));
                        self.holders_drawn += 1;
                    }
                }
                (Some(TgeClass::Projectile), TgeRecord::Projectile(p)) => {
                    seen.push(index);
                    if p.explosion.is_some() {
                        if let Some(mut trail) = self.bolts.remove(&index).and_then(|b| b.trail) {
                            trail.dead = true;
                            self.fading.push(trail);
                        }
                        continue;
                    }
                    let bolt = match self.bolts.entry(index) {
                        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            let (Some(start), Some(initial)) = (ghost.position, p.initial.as_ref())
                            else {
                                continue;
                            };
                            let velocity = initial.velocity.map_or([0.0; 3], |v| {
                                v.value(torque::ghosts::projectile::VELOCITY_NORMAL_BITS)
                            });
                            e.insert(Bolt {
                                start,
                                velocity,
                                age: 0.0,
                                trail: bolt_trail(world, db),
                            })
                        }
                    };
                    let from = bolt.position();
                    bolt.age += dt;
                    let (at, v, age) = (bolt.position(), bolt.velocity, bolt.age);
                    if let Some(trail) = bolt.trail.as_mut()
                        && ms > 0
                    {
                        trail.advance(ms, &mut self.live);
                        trail.emit_trail(from, at, v, ms, &mut rng, &mut self.live);
                    }
                    let Some(file) = world.datablock_text(db, "projectileShapeName") else {
                        continue;
                    };
                    let Some(shape) = self.shape(assets, file) else {
                        continue;
                    };
                    let Some(instance) = shape.instance() else {
                        continue;
                    };
                    let yaw = v[0].atan2(v[1]);
                    let pitch = -v[2].atan2((v[0] * v[0] + v[1] * v[1]).sqrt());
                    let spin = Mat::from_axis_angle([0.0, 1.0, 0.0], BOLT_SPIN * age);
                    let mut m =
                        Mat::mul(&Mat::mul(&Mat::euler_z(yaw), &Mat::euler_x(pitch)), &spin);
                    m.set_column3(at);
                    out.push(view.shape_object(assets, &shape, &instance, &m, object_id(index, 2)));
                    self.bolts_drawn += 1;
                }
                _ => {}
            }
        }
        self.cubes.retain(|i, _| seen.contains(i));
        self.figures.retain(|i, _| seen.contains(i));
        self.rng = Some(rng);
        let gone: Vec<u32> = self
            .bolts
            .keys()
            .filter(|i| !seen.contains(i))
            .copied()
            .collect();
        for i in gone {
            if let Some(mut trail) = self.bolts.remove(&i).and_then(|b| b.trail) {
                trail.dead = true;
                self.fading.push(trail);
            }
        }
        out.extend(self.cubes.values().map(|c| c.object.clone()));
        self.draw_trails(view, assets);
        out
    }

    /// The trails' particles into `view`, facing the frame's eye.
    fn draw_trails(&mut self, view: &mut WorldView, assets: &KitAssets) {
        let eye = self.eye();
        let camera = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
        let (right, up) = (camera.column(0), camera.column(2));
        let mut set = TextureSet::new(assets);
        let mut draws = Vec::new();
        self.trail_particles = 0;
        let trails = self
            .bolts
            .values()
            .filter_map(|b| b.trail.as_ref())
            .chain(&self.fading);
        for e in trails {
            let (Some(centre), Some(d)) = (e.centre(), e.draw(right, up, &mut set, "trail")) else {
                continue;
            };
            let dist = (0..3)
                .map(|k| (centre[k] - eye.position[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            self.trail_particles += e.particles.len();
            draws.push((dist, d));
        }
        view.set_particles(&draws, set);
    }

    /// The eye: at the own figure's eye, else over the arena.
    pub fn eye(&self) -> Eye {
        match self.own {
            Some(o) => Eye {
                position: o.eye,
                pitch: o.pitch,
                yaw: o.yaw,
                fov: Some(FOV),
            },
            None => self.overview_eye(),
        }
    }

    /// An eye looking at the whole arena from above and to its south.
    pub fn overview_eye(&self) -> Eye {
        let mut eye = look_at([0.0, -34.0, 22.0], [0.0, 0.0, 0.0]);
        eye.fov = Some(FOV);
        eye
    }
}

impl Figure {
    /// A new record's position (or none): the figure moves from where it is drawn to it over a tick; a jump of more than a few
    /// units is a respawn, with no motion between.
    fn move_to(&mut self, at: [f32; 3], dt: f32) {
        if at != self.to {
            let moved = (0..3)
                .map(|i| (at[i] - self.to[i]).powi(2))
                .sum::<f32>()
                .sqrt();
            self.from = if moved > 4.0 { at } else { self.position() };
            self.to = at;
            self.since = 0.0;
        } else {
            self.since += dt;
        }
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
    let center = m.column(3);
    let top = center[2] + scale[2].abs() * 0.5;
    let tint = cube_tint(top);
    let object = view.shape_object_with(
        assets,
        shape,
        &instance,
        &m,
        scale,
        object_id(index, 0),
        &mut |d| retexture(d, tint),
    );
    Some(KeptCube { pose, object })
}
