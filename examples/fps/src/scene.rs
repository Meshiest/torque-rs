//! The client's ghosts as the world view's objects, and the eye.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say, their texture coordinates made from where each vertex is (one
//!   grid tile a unit) and tinted by height. A cube's object is kept until its record moves it. A map that asks for sharp
//!   texels ([`TextureFilter::Nearest`]) has its cubes magnified with nearest-neighbor filtering; minified, they stay linear
//!   between and within the mip levels, so the far floor does not shimmer.
//! - **Figures** (Players) of other players move on between their records along the last one's velocity, and a record that puts
//!   one elsewhere is warped to over a few ticks ([`crate::motion`]); they face the yaw and look at the head's pitch the same way,
//!   animated as TGE's client animates a Player ([`torque::client::player_anim`]) at the velocity they are drawn moving at. A figure that holds an image (its record's first
//!   image slot) plays the kit's `hold` on its arm thread, and the image's shape is drawn in its hand: the figure's `mount0`
//!   node times the inverse of the image's `mountPoint` (`ShapeBase::getRenderImageTransform`). A new fire count plays the
//!   image's `fire`, kicks the pistol up and back for a moment ([`recoil`]: in first person the view's pistol, in third the
//!   pistol in the hand), and flashes at its `muzzlePoint`: a burst of the particles of the emitter its datablock's fire state
//!   names (`stateEmitter`), thrown out along the barrel. The action the server set (a death) plays as the record carries it,
//!   and the head and the aiming arm follow the look.
//! - **The own figure** (the ghost the server named, [`OwnLook`]) stands where the client predicts it and is not seen: the eye is
//!   at its `eye` node, turned by the predicted yaw and head (the server runs the same moves, so the view is where the server
//!   aims, and drawn ahead by the mouse's waiting turn, so it does not wait for the next tick), and the
//!   pistol is drawn at the eye times the image's `eyeOffset` (a first-person image, casting no shadow). The figure and the
//!   pistol in its hand are still posed as a third-person view would pose them, drawn into the shadow maps only
//!   ([`material::SHADOW_ONLY`]): the own shadow falls on the floor.
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
use torque::client::player_anim::{PlayerAnimation, PlayerMotion, ServerAction};
use torque::ghost_world::world::Pose;
use torque::ghosts::{TgeClass, TgeRecord, Value};
use torque::shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use torque_example_common::KitAssets;
use torque_example_common::view::{SceneShape, WorldView};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{DynamicObject, material};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::particles::{Emitter, Rng};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Filter, Vertex};

use crate::motion::{GhostMotion, Update};
/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;
/// The eye's height above the feet when the shape has no `eye` node.
pub const EYE_HEIGHT: f32 = 1.75;
/// The field of view (degrees, horizontal).
pub const FOV: u8 = 90;
/// How fast a bolt turns about its flight (radians a second).
pub const BOLT_SPIN: f32 = std::f32::consts::TAU;
/// The muzzle flash's burst: how many particles, about how far from the muzzle (m).
pub const FLASH_PARTICLES: u32 = 8;
pub const FLASH_RADIUS: f32 = 0.02;
/// The recoil's kick: how long it takes to reach its height and how fast it settles after (seconds), and how far it goes at
/// its height: back (m) and up (radians), in first person and in the hand.
pub const RECOIL_RISE: f32 = 0.04;
pub const RECOIL_SETTLE: f32 = 0.08;
pub const RECOIL_BACK: f32 = 0.06;
pub const RECOIL_UP: f32 = 0.14;
pub const RECOIL_UP_HAND: f32 = 0.3;

/// How far into its kick the recoil is `t` seconds after a shot: 0 at rest, 1 at its height (a quick rise, then a settle).
pub fn recoil(t: f32) -> f32 {
    if !(0.0..1.0).contains(&t) {
        0.0
    } else if t < RECOIL_RISE {
        t / RECOIL_RISE
    } else {
        (-(t - RECOIL_RISE) / RECOIL_SETTLE).exp()
    }
}

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

/// How a map's textures are drawn: the map's `textureFilter` (`$Arena::TextureFilter` once the server named it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextureFilter {
    /// Linear filtering, the engine's default.
    #[default]
    Smooth,
    /// Sharp texels: nearest-neighbor filtering where a texel covers more than a pixel.
    Nearest,
}

impl TextureFilter {
    /// The filter a map names (`nearest`, any case); anything else is smooth.
    pub fn named(name: &str) -> TextureFilter {
        if name.trim().eq_ignore_ascii_case("nearest") {
            TextureFilter::Nearest
        } else {
            TextureFilter::Smooth
        }
    }
}

/// `o` with the material bits `bits` on each of its draws.
fn marked(mut o: DynamicObject, bits: u32) -> DynamicObject {
    if bits == 0 {
        return o;
    }
    o.draws = o.draws.into_iter().map(|d| d.with_material(bits)).collect();
    o
}

fn retexture(d: &mut Draw, tint: [u8; 3], filter: TextureFilter) {
    for v in &mut d.vertices {
        v.uv = world_uv(v);
        v.color = [tint[0], tint[1], tint[2], 255];
    }
    if let Some((_, sampler)) = d.texture.as_mut() {
        sampler.repeat = true;
        if filter == TextureFilter::Nearest {
            sampler.mag = Filter::Nearest;
        }
    }
}

/// A cube's object, kept with the pose it was made at.
struct KeptCube {
    pose: Pose,
    object: DynamicObject,
}

/// A held image: its shape, its instance, its `fire` thread, the fire count last seen, the seconds since it changed and the
/// flash of a shot not drawn yet.
struct Held {
    shape: SceneShape,
    instance: ShapeInstance,
    fire: Option<ThreadId>,
    fire_count: Option<u32>,
    since_fire: f32,
    flash: Option<Emitter>,
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
            since_fire: f32::MAX,
            flash: None,
            mount_inverse,
            eye_offset,
        })
    }

    /// The frame: a new fire count restarts `fire` and the recoil, then `dt` of it. Whether the count was new (a shot).
    fn update(&mut self, fire_count: u32, dt: f32) -> bool {
        let shape = self.shape.shape();
        let fired = self.fire_count.is_some_and(|c| c != fire_count);
        if fired {
            self.since_fire = 0.0;
        } else {
            self.since_fire += dt;
        }
        if let Some(t) = self.fire {
            if fired && let Some(seq) = find_sequence(shape, b"fire") {
                self.instance.set_sequence(shape, t, seq as i32, 0.0);
            }
            self.instance.advance_time(shape, t, dt);
        }
        self.fire_count = Some(fire_count);
        let detail = self.shape.detail().unwrap_or(0);
        let _ = self.instance.animate(shape, detail, None, &mut |_| true);
        fired
    }

    /// The kick of the recoil now (0 to 1).
    fn kick(&self) -> f32 {
        recoil(self.since_fire)
    }

    /// The muzzle's place and the barrel's direction with the image drawn at `m`.
    fn muzzle(&self, m: &Mat) -> ([f32; 3], [f32; 3]) {
        let at = self
            .shape
            .node("muzzlePoint")
            .and_then(|n| self.instance.node_transform(n))
            .map_or([0.0; 3], |n| [n[3], n[7], n[11]]);
        (m.mul_p(at), m.mul_v([0.0, 1.0, 0.0]))
    }
}

/// The emitter of an image's muzzle flash: the `stateEmitter` of its datablock's fire state (the state with `stateFire`), when
/// the client holds it with its particles. The datablock's record lists each state's members in order (`state` first, then
/// `fire`, ..., `emitter` when the state has one).
pub fn muzzle_flash(world: &TgeWorld, image: u32) -> Option<Emitter> {
    let db = world.datablocks.get(&image)?;
    let mut firing = false;
    for (name, _, value) in &db.fields {
        match (name.as_str(), value) {
            ("state", _) => firing = false,
            ("fire", Value::Bool(fire)) => firing = *fire,
            ("emitter", Value::Id(Some(id))) if firing => {
                return Emitter::new(&world.records, *id);
            }
            _ => {}
        }
    }
    None
}

/// A figure: where it is drawn, its motion between records (another player's) and its animation.
struct Figure {
    shape: SceneShape,
    motion: GhostMotion,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    /// The client's own figure (drawn where the client predicts it, into the shadow maps only).
    own: bool,
    instance: ShapeInstance,
    anim: PlayerAnimation,
    held: Option<Held>,
    held_db: Option<u32>,
}

impl Figure {
    fn position(&self) -> [f32; 3] {
        self.position
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

/// Which ghost is the client's own figure, where the client predicts it and where its view is turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnLook {
    pub ghost: u32,
    pub yaw: f32,
    /// Positive looks down.
    pub pitch: f32,
    /// The feet as the client predicts them (none before the prediction starts: the record's).
    pub position: Option<[f32; 3]>,
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
    /// How the cubes are drawn.
    filter: TextureFilter,
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
    /// The muzzle flashes started so far.
    pub flashes: usize,
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

    /// Draws the cubes with `filter` from now on (the ones made with another are made again).
    pub fn set_texture_filter(&mut self, filter: TextureFilter) {
        if filter != self.filter {
            self.filter = filter;
            self.cubes.clear();
        }
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

    /// The figures moved on by `dt` seconds to `world`'s Player records, with nothing drawn: the own one where the client predicts
    /// it, the others between their records ([`crate::motion`]); each animated and its held image posed, and the own view set.
    pub fn update_figures(
        &mut self,
        assets: &KitAssets,
        world: &TgeWorld,
        own: Option<OwnLook>,
        dt: f32,
    ) {
        self.own = None;
        let mut seen = Vec::new();
        for (&index, ghost) in &world.records.ghosts {
            let (Some(TgeClass::Player), Some(record @ TgeRecord::Player(_))) =
                (ghost.class, ghost.record.as_ref())
            else {
                continue;
            };
            let Some(db) = ghost.datablock() else {
                continue;
            };
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
            let head =
                torque::ghost_world::tge::head_fractions(record).map_or(0.0, |h| h[0]) * max_look;
            let mut motion = PlayerMotion::of_record(record, yaw).unwrap_or(PlayerMotion {
                yaw,
                ..PlayerMotion::default()
            });
            motion.server_action = ServerAction::of_record(record);
            let update = Update {
                position,
                velocity: motion.velocity,
                yaw,
                pitch: head,
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
                    let Some(anim) = PlayerAnimation::new(shape.shape(), &mut instance) else {
                        continue;
                    };
                    e.insert(Figure {
                        shape: shape.clone(),
                        motion: GhostMotion::new(update),
                        position,
                        yaw,
                        pitch: head,
                        own: false,
                        instance,
                        anim,
                        held: None,
                        held_db: None,
                    })
                }
            };
            figure.own = look.is_some();
            match look {
                Some(o) => {
                    // Kept at the record, so the figure goes on from there if it stops being the own one.
                    figure.motion = GhostMotion::new(update);
                    figure.position = o.position.unwrap_or(position);
                    figure.yaw = o.yaw;
                    figure.pitch = o.pitch;
                    if let Some(c) = world.control_state.as_ref() {
                        motion.velocity = c.velocity;
                        motion.falling = c.falling;
                    }
                }
                None => {
                    figure.motion.receive(update);
                    figure.motion.advance(dt);
                    figure.position = figure.motion.position();
                    figure.yaw = figure.motion.yaw();
                    figure.pitch = figure.motion.pitch();
                    motion.velocity = figure.motion.velocity();
                }
            }
            motion.yaw = figure.yaw;
            // The held image.
            if image.map(|i| i.0) != figure.held_db {
                figure.held_db = image.map(|i| i.0);
                figure.held = held_shape.and_then(|(shape, offset)| Held::new(shape, offset));
            }
            let hold = figure
                .held
                .as_ref()
                .and_then(|_| figure.anim.actions().iter().position(|a| a.name == b"hold"));
            motion.arm_action = hold.and_then(|h| u32::try_from(h).ok());
            motion.look = ((figure.pitch / max_look + 1.0) * 0.5).clamp(0.0, 1.0);
            let s = figure.shape.shape();
            figure.anim.update(s, &mut figure.instance, &motion, dt);
            let detail = figure.shape.detail().unwrap_or(0);
            let _ = figure.instance.animate(s, detail, None, &mut |_| true);
            if let (Some(held), Some((db, fire_count))) = (figure.held.as_mut(), image)
                && held.update(fire_count, dt)
            {
                held.flash = muzzle_flash(world, db);
            }
            if figure.own {
                let eye = figure.eye_transform();
                self.own = Some(OwnView {
                    position: figure.position,
                    yaw: figure.yaw,
                    pitch: figure.pitch,
                    eye: eye.column(3),
                });
            }
        }
        self.figures.retain(|i, _| seen.contains(i));
    }

    /// The figures' objects as [`ArenaScene::update_figures`] left them: the own one and the pistol in its hand into the shadow
    /// maps only, and its first-person pistol at the eye.
    fn figure_objects(&mut self, view: &mut WorldView, assets: &KitAssets) -> Vec<DynamicObject> {
        let mut out = Vec::new();
        let mut holders = 0;
        // The shots' flashes: each at the muzzle of the pistol the frame draws (the own figure's first-person one).
        let mut flashes = Vec::new();
        for (&index, figure) in &mut self.figures {
            let body = figure.transform();
            let seen_bits = if figure.own { material::SHADOW_ONLY } else { 0 };
            let eye = figure.eye_transform();
            if figure.own
                && let Some(held) = figure.held.as_mut()
            {
                // The pistol at the eye's offset, kicked back and up while it recoils.
                let kick = held.kick();
                let mut offset = Mat::euler_x(-RECOIL_UP * kick);
                let [x, y, z] = held.eye_offset;
                offset.set_column3([x, y - RECOIL_BACK * kick, z + 0.2 * RECOIL_BACK * kick]);
                let m = Mat::mul(&eye, &offset);
                if let Some(e) = held.flash.take() {
                    let (at, dir) = held.muzzle(&m);
                    flashes.push((e, at, dir));
                }
                let image = view.shape_object(assets, &held.shape, &held.instance, &m, 1);
                out.push(marked(image, material::VIEW_ONLY));
            }
            let figure_object = view.shape_object(
                assets,
                &figure.shape,
                &figure.instance,
                &body,
                object_id(index, 0),
            );
            out.push(marked(figure_object, seen_bits));
            if let (Some(held), Some(hand)) = (
                figure.held.as_mut(),
                figure
                    .shape
                    .node("mount0")
                    .and_then(|n| figure.instance.node_transform(n)),
            ) {
                let kick = Mat::euler_x(-RECOIL_UP_HAND * held.kick());
                let hand = Mat::mul(&Mat::mul(&body, &Mat { m: *hand }), &kick);
                let m = Mat::mul(&hand, &held.mount_inverse);
                if let Some(e) = held.flash.take() {
                    let (at, dir) = held.muzzle(&m);
                    flashes.push((e, at, dir));
                }
                let held_object =
                    view.shape_object(assets, &held.shape, &held.instance, &m, object_id(index, 1));
                out.push(marked(held_object, seen_bits));
                if !figure.own {
                    holders += 1;
                }
            }
        }
        self.holders_drawn = holders;
        let mut rng = self.rng.take().unwrap_or_else(|| Rng::new(0x5eed));
        for (mut e, at, dir) in flashes {
            e.emit_burst(
                at,
                dir,
                FLASH_RADIUS,
                FLASH_PARTICLES,
                &mut rng,
                &mut self.live,
            );
            e.dead = true;
            self.fading.push(e);
            self.flashes += 1;
        }
        self.rng = Some(rng);
        out
    }

    /// The frame's objects: `world`'s cubes, the figures as [`ArenaScene::update_figures`] left them, and the bolts moved on by
    /// `dt` seconds.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
        dt: f32,
    ) -> Vec<DynamicObject> {
        let mut seen = Vec::new();
        let mut out = self.figure_objects(view, assets);
        self.bolts_drawn = 0;
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
                    if let Some(cube) = make_cube(view, assets, &shape, pose, index, self.filter) {
                        self.cubes.insert(index, cube);
                    }
                }
                (Some(TgeClass::Player), TgeRecord::Player(_)) => {}
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
            let (Some(center), Some(d)) = (e.centre(), e.draw(right, up, &mut set, "trail")) else {
                continue;
            };
            let dist = (0..3)
                .map(|k| (center[k] - eye.position[k]).powi(2))
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

fn make_cube(
    view: &mut WorldView,
    assets: &KitAssets,
    shape: &SceneShape,
    pose: Pose,
    index: u32,
    filter: TextureFilter,
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
        &mut |d| retexture(d, tint, filter),
    );
    Some(KeptCube { pose, object })
}

#[cfg(test)]
mod tests {
    use torque_render_scene::{Blend, Cull, Sampler, TexEnv};

    use super::*;

    fn textured() -> Draw {
        let linear = Sampler {
            repeat: false,
            mag: Filter::Linear,
            min: Filter::Linear,
            mip: Filter::Linear,
            mipmaps: true,
        };
        Draw {
            label: String::new(),
            vertices: Vec::new(),
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture: Some((0, linear)),
            blend: Blend::Opaque,
            cull: Cull::None,
            depth_test: true,
            depth_write: true,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        }
    }

    #[test]
    fn the_recoil_kicks_quickly_and_settles() {
        assert_eq!(recoil(0.0), 0.0);
        assert_eq!(recoil(RECOIL_RISE), 1.0);
        assert!(recoil(RECOIL_RISE * 0.5) > 0.4);
        let later = [0.08, 0.15, 0.3].map(recoil);
        assert!(
            later[0] > later[1] && later[1] > later[2] && later[2] < 0.05,
            "{later:?}"
        );
        assert_eq!(recoil(2.0), 0.0);
        assert_eq!(recoil(f32::MAX), 0.0);
    }

    #[test]
    fn a_map_names_its_texture_filter() {
        assert_eq!(TextureFilter::named("nearest"), TextureFilter::Nearest);
        assert_eq!(TextureFilter::named(" Nearest "), TextureFilter::Nearest);
        assert_eq!(TextureFilter::named(""), TextureFilter::Smooth);
        assert_eq!(TextureFilter::named("linear"), TextureFilter::Smooth);
    }

    #[test]
    fn sharp_cubes_are_magnified_nearest_and_minified_linear() {
        let mut d = textured();
        retexture(&mut d, [255; 3], TextureFilter::Nearest);
        let (_, s) = d.texture.unwrap();
        assert!(s.repeat && s.mipmaps);
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Nearest, Filter::Linear, Filter::Linear)
        );

        let mut d = textured();
        retexture(&mut d, [255; 3], TextureFilter::Smooth);
        let (_, s) = d.texture.unwrap();
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Linear, Filter::Linear, Filter::Linear)
        );
    }
}
