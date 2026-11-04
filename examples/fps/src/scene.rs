//! The client's ghosts as the world view's objects, and the eye.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say, their texture coordinates made from where each vertex is (one
//!   grid tile a unit) and tinted by height. A cube's object is kept until its record moves it. A map that asks for sharp
//!   texels ([`TextureFilter::Nearest`]) has its cubes magnified with nearest-neighbor filtering; minified, they stay linear
//!   between and within the mip levels, so the far floor does not shimmer.
//! - **Figures** (Players) of other players stand where the client runs them between their records (the client's
//!   [`GhostPlayers`]: on along the last one's velocity, warped to each new one over a few ticks), facing that yaw and looking
//!   at that head's pitch, animated as TGE's client animates a Player ([`torque::client::player_anim`]) at the velocity they are
//!   drawn moving at. A figure that holds an image (its record's first
//!   image slot) plays the kit's `hold` on its arm thread, and the image's shape is drawn in its hand: the figure's `mount0`
//!   node times the inverse of the image's `mountPoint` (`ShapeBase::getRenderImageTransform`). A new fire count plays the
//!   image's `fire`, kicks the pistol up and back for a moment ([`recoil`]: in first person the view's pistol, in third the
//!   pistol in the hand), and flares at its `muzzlePoint` for [`FLARE_SECONDS`]: a star facing the eye. The emitter its datablock's fire state names (`stateEmitter`) emits from the
//!   muzzle for its `stateEmitterTime`, following the muzzle as the pistol moves (`ShapeBaseImage`'s state emitter). The action the server set (a death) plays as the record carries it,
//!   and the head and the aiming arm follow the look.
//! - **The own figure** (the ghost the server named, [`OwnLook`]) stands where the client predicts it and is not seen: the eye is
//!   at its `eye` node, turned by the predicted yaw and head (the server runs the same moves, so the view is where the server
//!   aims, and drawn ahead by the mouse's waiting turn, so it does not wait for the next tick), and the
//!   pistol is drawn at the eye times the image's `eyeOffset` (a first-person image, casting no shadow). The figure and the
//!   pistol in its hand are still posed as a third-person view would pose them, drawn into the shadow maps only
//!   ([`material::SHADOW_ONLY`]): the own shadow falls on the floor.
//! - **Bolts** (Projectiles) fly on from their first record along its velocity (the client runs them on its own, as TGE's
//!   client runs a projectile ghost) and are drawn along it, turning slowly about their flight, until their explosion record
//!   hides them. The server fires a bolt along its shooter's eye line (so it goes where the crosshair is); the client draws it
//!   leaving the shooter's pistol muzzle (growing out of it over its first [`BOLT_GROW`] units) and curving onto that line
//!   over its first [`BOLT_BLEND`] units. A bolt whose datablock names a `particleEmitter` leaves a trail of its particles
//!   behind it ([`Emitter::emit_trail`] over each frame's flight, as `Projectile::processTick` emits on the client), so the
//!   trail draws the curve from the muzzle; a gone bolt's trail is left to fade out (`deleteWhenEmpty`). With
//!   [`ArenaScene::ribbon_trails`] (the default) that trail is a continuous ribbon instead ([`Ribbon`], its look the
//!   emitter's particle data's): a point at each frame's drawn place, from the muzzle, ending where the bolt burst. A bolt that bursts
//!   on the world (not on a figure) leaves one of its datablock's decals there ([`DecalManager`]: at most [`MAX_DECALS`],
//!   each for [`DECAL_SECONDS`]).
//! - **Aiming down the sights** ([`ArenaScene::update_aim`]): while the client aims, the first-person pistol moves from its
//!   `eyeOffset` to where its `sightRear` and `sightFront` nodes lie on the eye's line ([`AIM_REAR_DISTANCE`] ahead) over
//!   [`AIM_SECONDS`], eased, and the field of view narrows from [`FOV`] to [`AIM_FOV`] with it ([`ArenaScene::turn_scale`]
//!   slows the mouse's turn by as much). A figure whose record's arm action is the shape's `aim` (the server's, while its
//!   player aims) holds its pistol up at its eye.
//!
//! Credit: the image's hand transform follows `ShapeBase::getRenderImageTransform` in Torque3D's MIT-licensed
//! `T3D/shapeBase.cpp`, and the hit burst follows `Explosion::explode` in `T3D/fx/explosion.cpp` (each Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this example's own.

use std::collections::{BTreeMap, HashMap};

use torque::app::view::{SceneShape, WorldView};
use torque::client::conn::tge::TgeWorld;
use torque::client::player_anim::{PlayerAnimation, PlayerMotion, ServerAction};
use torque::ghost_world::world::Pose;
use torque::ghosts::{TgeClass, TgeRecord, Value};
use torque::shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use torque_example_common::KitAssets;
use torque_render_scene::decals::{DecalManager, Triangle, projectile_decal};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{DynamicObject, material};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::particles::{Emitter, Rng};
use torque_render_scene::ribbons::{Ribbon, projectile_ribbon};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Filter, Vertex};

use torque::client::predict::ghost_players::GhostPlayers;
/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;
/// The eye's height above the feet when the shape has no `eye` node.
pub const EYE_HEIGHT: f32 = 1.75;
/// The field of view (degrees, horizontal).
pub const FOV: u8 = 90;
/// Aiming down the sights: how long the pistol takes to come up to the sights (seconds), the field of view there (degrees,
/// horizontal), and how far ahead of the eye the rear sight is held. The pistol is placed there from its `sightRear` and
/// `sightFront` nodes: turned so the line through them runs along the eye's, and moved so that line is the eye's (both
/// sights on the crosshair). A pistol without them is held at [`AIM_OFFSET`] (the image's place in the eye's frame).
pub const AIM_SECONDS: f32 = 0.18;
pub const AIM_FOV: f32 = 60.0;
pub const AIM_REAR_DISTANCE: f32 = 0.36;
pub const AIM_OFFSET: [f32; 3] = [0.0, 0.42, -0.057];

/// The ease of the sights' blend: `t` (0 to 1 of [`AIM_SECONDS`]) smoothed at both ends.
pub fn aim_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How fast a bolt turns about its flight (radians a second).
pub const BOLT_SPIN: f32 = std::f32::consts::TAU;
/// How far a bolt flies before it is drawn on its flight (units): until then it is drawn curving there from the shooter's muzzle
/// (the server fires it along the eye's line, so it goes where the crosshair is).
pub const BOLT_BLEND: f32 = 12.0;
/// A bolt leaving the muzzle grows from this share of its size to its whole size over its first [`BOLT_GROW`] units, so it
/// comes out of the barrel instead of filling the view beside it.
pub const BOLT_START_SIZE: f32 = 0.3;
pub const BOLT_GROW: f32 = 1.5;
/// How far from a figure's eye a bolt may have been fired and still be that figure's (units).
pub const SHOOTER_REACH: f32 = 1.5;

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
    /// The emitters its shots started (the fire state's `stateEmitter`), each with the milliseconds it still emits, following
    /// the muzzle.
    emitters: Vec<(Emitter, f32)>,
    /// The frame time the emitters have not run yet (ms, under one).
    emit_ms: f32,
    /// Seconds since the last shot (its flare shows for [`FLARE_SECONDS`]).
    since_flare: f32,
    /// The muzzle's place and the barrel's direction as the last frame drew the image.
    muzzle_at: Option<([f32; 3], [f32; 3])>,
    /// The image's `mountPoint` node at rest, inverted.
    mount_inverse: Mat,
    /// The datablock's `eyeOffset`.
    eye_offset: [f32; 3],
    /// Where the image is held aiming down the sights (its place in the eye's frame), and how far it is turned down there
    /// (radians) so its sight line is level.
    aim_offset: [f32; 3],
    aim_pitch: f32,
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
        let node_at = |name: &str| {
            shape
                .node(name)
                .and_then(|n| instance.node_transform(n))
                .map(|m| [m[3], m[7], m[11]])
        };
        let (aim_offset, aim_pitch) = match (node_at("sightRear"), node_at("sightFront")) {
            (Some(rear), Some(front)) => {
                let pitch = (front[2] - rear[2]).atan2(front[1] - rear[1]);
                let r = Mat::euler_x(pitch).mul_v(rear);
                ([-r[0], AIM_REAR_DISTANCE - r[1], -r[2]], pitch)
            }
            _ => (AIM_OFFSET, 0.0),
        };
        Some(Held {
            shape,
            instance,
            fire,
            fire_count: None,
            since_fire: f32::MAX,
            emitters: Vec::new(),
            emit_ms: 0.0,
            since_flare: f32::MAX,
            muzzle_at: None,
            mount_inverse,
            eye_offset,
            aim_offset,
            aim_pitch,
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

/// The emitter an image's shot starts: the `stateEmitter` of its datablock's fire state (the state with `stateFire`), when
/// the client holds it with its particles, and how long it emits (`stateEmitterTime`, seconds). The datablock's record lists
/// each state's members in order (`state` first, then `fire`, ..., `emitter` and `emitterTime` when the state has one).
pub fn fire_emitter(world: &TgeWorld, image: u32) -> Option<(Emitter, f32)> {
    let db = world.datablocks.get(&image)?;
    let mut firing = false;
    let mut found: Option<Emitter> = None;
    for (name, _, value) in &db.fields {
        match (name.as_str(), value) {
            ("state", _) if found.is_some() => break,
            ("state", _) => firing = false,
            ("fire", Value::Bool(fire)) => firing = *fire,
            ("emitter", Value::Id(Some(id))) if firing => {
                found = Some(Emitter::new(&world.records, *id)?);
            }
            ("emitterTime", Value::F32(t)) if found.is_some() => {
                return found.map(|e| (e, t.clamp(0.0, MAX_EMITTER_SECONDS)));
            }
            _ => {}
        }
    }
    found.map(|e| (e, 0.0))
}

/// The longest an image's emitter emits after a shot (seconds; a server's datablock names it).
pub const MAX_EMITTER_SECONDS: f32 = 5.0;

/// A shot's flare at the muzzle: how long it shows (seconds) and the star seen head on (its width, units). It shrinks and fades
/// over the time.
pub const FLARE_SECONDS: f32 = 0.07;
pub const FLARE_STAR: f32 = 0.26;
/// The flare's texture.
pub const FLARE_STAR_TEXTURE: &str = "kit/flare_star";

/// A quad draw of `corners` (with `uv`s), added over what is behind it, `alpha` its strength.
fn sprite(label: &str, corners: [[f32; 3]; 4], texture: usize, alpha: f32) -> Draw {
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let v = |i: usize| Vertex {
        position: corners[i],
        normal: [0.0, 0.0, 1.0],
        color: [255, 255, 255, a],
        uv: uv[i],
    };
    Draw {
        label: label.to_string(),
        vertices: vec![v(0), v(1), v(2), v(0), v(2), v(3)],
        lighting: false,
        tex_env: torque_render_scene::TexEnv::Modulate,
        texture: Some((
            texture,
            torque_render_scene::Sampler {
                repeat: false,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        )),
        blend: torque_render_scene::Blend::Additive,
        cull: torque_render_scene::Cull::None,
        depth_test: true,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A shot's flare at the muzzle `at` along `dir`, `t` (0 to 1) through its time, seen from `eye` with the camera's `right` and
/// `up`: the star facing the eye at the muzzle.
fn flare_draws(
    at: [f32; 3],
    dir: [f32; 3],
    t: f32,
    eye: [f32; 3],
    (right, up): ([f32; 3], [f32; 3]),
    set: &mut TextureSet,
) -> Vec<(f32, Draw)> {
    let mut out = Vec::new();
    let fade = 1.0 - t.clamp(0.0, 1.0);
    let dist = (0..3).map(|k| (at[k] - eye[k]).powi(2)).sum::<f32>().sqrt();
    if let Some(tex) = set.get_modulated(FLARE_STAR_TEXTURE) {
        let h = 0.5 * FLARE_STAR * (0.45 + 0.55 * fade);
        let c = [0, 1, 2].map(|k| at[k] + dir[k] * 0.01);
        let p = |sx: f32, sy: f32| [0, 1, 2].map(|k| c[k] + (right[k] * sx + up[k] * sy) * h);
        let corners = [p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)];
        // Drawn twice: the additive passes add up to a brighter flash than the texture alone.
        out.push((dist, sprite("flare star", corners, tex, fade)));
        out.push((dist, sprite("flare star", corners, tex, fade)));
    }
    out
}

/// A figure: where it is drawn and its animation.
struct Figure {
    shape: SceneShape,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    /// The client's own figure (drawn where the client predicts it, into the shadow maps only).
    own: bool,
    instance: ShapeInstance,
    anim: PlayerAnimation,
    held: Option<Held>,
    held_db: Option<u32>,
    /// How far the first-person pistol is up at the sights (0 at the hip, 1 at the sights; eased).
    aim: f32,
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

    /// The first-person pistol's transform: at the eye times the image's `eyeOffset` (moved toward its place at the sights as
    /// they come up), kicked back and up while it recoils.
    fn view_image(&self, held: &Held) -> Mat {
        let kick = held.kick();
        let mut offset = Mat::mul(
            &Mat::euler_x(-RECOIL_UP * kick),
            &Mat::euler_x(held.aim_pitch * self.aim),
        );
        let [x, y, z] = [0, 1, 2]
            .map(|k| held.eye_offset[k] + (held.aim_offset[k] - held.eye_offset[k]) * self.aim);
        offset.set_column3([x, y - RECOIL_BACK * kick, z + 0.2 * RECOIL_BACK * kick]);
        Mat::mul(&self.eye_transform(), &offset)
    }

    /// The pistol in the hand's transform (the `mount0` node times the inverse of the image's `mountPoint`), kicked up while it
    /// recoils; none without the node.
    fn hand_image(&self, held: &Held) -> Option<Mat> {
        let hand = self
            .shape
            .node("mount0")
            .and_then(|n| self.instance.node_transform(n))?;
        let kick = Mat::euler_x(-RECOIL_UP_HAND * held.kick());
        let hand = Mat::mul(&Mat::mul(&self.transform(), &Mat { m: *hand }), &kick);
        Some(Mat::mul(&hand, &held.mount_inverse))
    }

    /// The pistol's `muzzlePoint` as the frame draws it: the own figure's first-person pistol, another's in its hand.
    fn muzzle(&self) -> Option<[f32; 3]> {
        let held = self.held.as_ref()?;
        let image = if self.own {
            self.view_image(held)
        } else {
            self.hand_image(held)?
        };
        Some(held.muzzle(&image).0)
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

/// Where a bolt is drawn, its velocity, its age and its size (a share of its shape's).
type BoltPose = ([f32; 3], [f32; 3], f32, f32);

/// A bolt: where its first record put it and when, and its trail.
struct Bolt {
    db: u32,
    start: [f32; 3],
    velocity: [f32; 3],
    age: f32,
    /// Where it is drawn from at first, less where it is (the shooter's muzzle beside the eye's line), taken back over its
    /// first [`BOLT_BLEND`] units.
    offset: [f32; 3],
    trail: Option<Emitter>,
    /// Its trail drawn as a ribbon (in place of `trail`).
    ribbon: Option<Ribbon>,
}

impl Bolt {
    /// Where the bolt is (its flight along the line it was fired on).
    fn position(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| self.start[i] + self.velocity[i] * self.age)
    }

    /// How far it has flown (units).
    fn flown(&self) -> f32 {
        self.velocity.iter().map(|v| v * v).sum::<f32>().sqrt() * self.age
    }

    /// Its size as drawn: growing out of the muzzle ([`BOLT_START_SIZE`], [`BOLT_GROW`]).
    fn size(&self) -> f32 {
        BOLT_START_SIZE + (1.0 - BOLT_START_SIZE) * (self.flown() / BOLT_GROW).clamp(0.0, 1.0)
    }

    /// Where it is drawn: from the muzzle, curving onto its flight ([`BOLT_BLEND`]).
    fn drawn(&self) -> [f32; 3] {
        let k = (1.0 - (self.flown() / BOLT_BLEND).clamp(0.0, 1.0)).powi(2);
        let at = self.position();
        [0, 1, 2].map(|i| at[i] + self.offset[i] * k)
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

/// The burst of a bolt of ProjectileData `db` hitting a figure at `at`: its `explosion`'s `particleEmitter`, `particleDensity`
/// particles within `particleRadius` (`Explosion::explode`), when the client holds them.
pub fn hit_burst(
    world: &TgeWorld,
    db: u32,
    at: [f32; 3],
    normal: [f32; 3],
    rng: &mut Rng,
    live: &mut usize,
) -> Option<Emitter> {
    let id = |db: u32, field: &str| match world.datablocks.get(&db)?.get(field, 0)? {
        Value::Id(id) => *id,
        Value::Int(v) => u32::try_from(*v).ok(),
        _ => None,
    };
    let number =
        |db: u32, field: &str| match world.datablocks.get(&db).and_then(|d| d.get(field, 0)) {
            Some(Value::F32(v)) => Some(*v),
            Some(Value::Int(v)) => Some(*v as f32),
            _ => None,
        };
    let explosion = id(db, "explosion")?;
    let mut e = Emitter::new(&world.records, id(explosion, "particleEmitter")?)?;
    let count = number(explosion, "particleDensity")
        .unwrap_or(10.0)
        .clamp(0.0, 256.0) as u32;
    let radius = number(explosion, "particleRadius").unwrap_or(0.0);
    e.emit_burst(at, normal, radius, count, rng, live);
    e.dead = true;
    Some(e)
}

/// The most hit bursts [`ArenaScene::hit_bursts`] keeps.
pub const MAX_HIT_BURSTS: usize = 16;

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
    /// The trails of the bolts gone and the hits' bursts, fading out.
    fading: Vec<Emitter>,
    /// The bolts whose hit on a figure burst (each bursts once).
    burst: std::collections::BTreeSet<u32>,
    /// Where the last hits on figures burst (the last [`MAX_HIT_BURSTS`]).
    pub hit_bursts: Vec<[f32; 3]>,
    /// The holes the bolts left in the world.
    pub decals: DecalManager,
    rng: Option<Rng>,
    /// The live particles of every trail.
    live: usize,
    /// The frame time the trails have not run yet (ms, under one).
    trail_ms: f32,
    own: Option<OwnView>,
    /// How far the sights' blend has run (0 at the hip, 1 at the sights; not eased).
    aim: f32,
    /// The bolts drawn in the last frame, and the other figures holding an image.
    pub bolts_drawn: usize,
    pub holders_drawn: usize,
    /// The trail particles drawn in the last frame.
    pub trail_particles: usize,
    /// Whether bolts' trails are drawn as ribbons (their emitter's particle data along a strip) rather than particles.
    pub ribbon_trails: bool,
    /// The trails drawn as ribbons in the last frame, and the gone bolts' ribbons, aging away.
    pub ribbons_drawn: usize,
    fading_ribbons: Vec<Ribbon>,
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

/// The most bolt holes kept, how long each stays and how long of that it fades out (seconds).
pub const MAX_DECALS: usize = 64;
pub const DECAL_SECONDS: f32 = 25.0;
pub const DECAL_FADE: f32 = 4.0;

impl ArenaScene {
    /// A scene with nothing in it yet.
    pub fn new() -> ArenaScene {
        ArenaScene {
            decals: DecalManager::new(MAX_DECALS, DECAL_SECONDS, DECAL_FADE),
            ribbon_trails: true,
            ..ArenaScene::default()
        }
    }

    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.cubes.clear();
        self.figures.clear();
        self.bolts.clear();
        self.fading.clear();
        self.burst.clear();
        self.hit_bursts.clear();
        self.decals.clear();
        self.fading_ribbons.clear();
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
        self.bolts.values().map(Bolt::drawn).collect()
    }

    /// Where the own figure's first-person pistol's sights (`sightFront`, `sightRear`) were drawn in the last frame.
    pub fn own_sights(&self) -> Option<[[f32; 3]; 2]> {
        let figure = self.figures.values().find(|f| f.own)?;
        let held = figure.held.as_ref()?;
        let m = figure.view_image(held);
        let at = |name: &str| {
            let n = held.shape.node(name)?;
            let p = held.instance.node_transform(n)?;
            Some(m.mul_p([p[3], p[7], p[11]]))
        };
        Some([at("sightFront")?, at("sightRear")?])
    }

    /// Where the own figure's first-person pistol's muzzle was drawn in the last frame.
    pub fn own_muzzle(&self) -> Option<[f32; 3]> {
        self.figures.values().find(|f| f.own)?.muzzle()
    }

    /// The muzzle of the figure whose eye is nearest `start` (within [`SHOOTER_REACH`]): the shooter's of a bolt fired there.
    fn shooter_muzzle(&self, start: [f32; 3]) -> Option<[f32; 3]> {
        let near = |f: &Figure| {
            let eye = f.eye_transform().column(3);
            (0..3)
                .map(|k| (eye[k] - start[k]).powi(2))
                .sum::<f32>()
                .sqrt()
        };
        self.figures
            .values()
            .map(|f| (near(f), f))
            .filter(|(d, _)| *d <= SHOOTER_REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .and_then(|(_, f)| f.muzzle())
    }

    /// The bolts moved on by `dt` seconds to `world`'s Projectile records, with nothing drawn: a new one drawn from its shooter's
    /// muzzle, an exploded or gone one's trail left to fade, the trails run.
    pub fn update_bolts(&mut self, world: &TgeWorld, dt: f32) {
        // The trails run whole milliseconds; the rest waits for the next frame.
        self.trail_ms += dt * 1000.0;
        let ms = self.trail_ms.floor().clamp(0.0, 1000.0) as u32;
        self.trail_ms = (self.trail_ms - ms as f32).clamp(0.0, 1.0);
        let mut rng = self.rng.take().unwrap_or_else(|| Rng::new(0x5eed));
        for e in &mut self.fading {
            e.advance(ms, &mut self.live);
        }
        self.decals.advance(dt);
        self.fading.retain(|e| !e.particles.is_empty());
        let mut seen = Vec::new();
        for (&index, ghost) in &world.records.ghosts {
            let (Some(TgeClass::Projectile), Some(TgeRecord::Projectile(p)), Some(db)) =
                (ghost.class, ghost.record.as_ref(), ghost.datablock())
            else {
                continue;
            };
            seen.push(index);
            if let Some(x) = p.explosion.as_ref() {
                if let Some(bolt) = self.bolts.remove(&index) {
                    let end = x.position.map(|v| v.get());
                    self.retire(bolt, Some(end));
                }
                use torque::collide::types::{PLAYER, VEHICLE};
                if x.hit_type != 0 && self.burst.insert(index) {
                    let at = x.position.map(|v| v.get());
                    let normal = x.normal.map(|v| v.get());
                    if x.hit_type & PLAYER != 0 {
                        if let Some(e) = hit_burst(world, db, at, normal, &mut rng, &mut self.live)
                        {
                            self.fading.push(e);
                            self.hit_bursts.push(at);
                            let cut = self.hit_bursts.len().saturating_sub(MAX_HIT_BURSTS);
                            self.hit_bursts.drain(..cut);
                        }
                    } else if x.hit_type & VEHICLE == 0
                        && let Some(d) = projectile_decal(world, db, &mut rng)
                    {
                        let angle = rng.f() * std::f32::consts::TAU;
                        let reach = d.size[0].max(d.size[1]);
                        let surface = cube_triangles_near(world, at, reach);
                        self.decals.add_clipped(&d, at, normal, angle, &surface);
                    }
                }
                continue;
            }
            if !self.bolts.contains_key(&index) {
                let (Some(start), Some(initial)) = (ghost.position, p.initial.as_ref()) else {
                    continue;
                };
                let velocity = initial.velocity.map_or([0.0; 3], |v| {
                    v.value(torque::ghosts::projectile::VELOCITY_NORMAL_BITS)
                });
                // The shooter is the figure whose eye is nearest where the bolt was fired (its record's age back along its
                // flight); it is drawn from that figure's muzzle as it stands now.
                let flown = initial.tick as f32 * torque::client::predict::ghost_players::TICK;
                let launch = [0, 1, 2].map(|k| start[k] - velocity[k] * flown);
                let offset = self
                    .shooter_muzzle(launch)
                    .map_or([0.0; 3], |m| [0, 1, 2].map(|k| m[k] - start[k]));
                self.bolts.insert(
                    index,
                    Bolt {
                        db,
                        start,
                        velocity,
                        age: 0.0,
                        offset,
                        trail: None,
                        ribbon: None,
                    },
                );
                continue;
            }
            let Some(bolt) = self.bolts.get_mut(&index) else {
                continue;
            };
            // The trail starts at the muzzle: as a ribbon when the scene draws trails so, else as the emitter's particles.
            if bolt.trail.is_none() && bolt.ribbon.is_none() {
                match projectile_ribbon(world, db).filter(|_| self.ribbon_trails) {
                    Some(data) => {
                        let mut r = Ribbon::new(data);
                        r.add_point(bolt.drawn());
                        bolt.ribbon = Some(r);
                    }
                    None => bolt.trail = bolt_trail(world, db),
                }
            }
            let from = bolt.drawn();
            bolt.age += dt;
            let (at, v) = (bolt.drawn(), bolt.velocity);
            if let Some(trail) = bolt.trail.as_mut()
                && ms > 0
            {
                trail.advance(ms, &mut self.live);
                trail.emit_trail(from, at, v, ms, &mut rng, &mut self.live);
            }
            if let Some(r) = bolt.ribbon.as_mut() {
                r.advance(dt * 1000.0);
                r.add_point(at);
            }
        }
        self.rng = Some(rng);
        self.burst.retain(|i| seen.contains(i));
        let gone: Vec<u32> = self
            .bolts
            .keys()
            .filter(|i| !seen.contains(i))
            .copied()
            .collect();
        for i in gone {
            if let Some(bolt) = self.bolts.remove(&i) {
                self.retire(bolt, None);
            }
        }
        for r in &mut self.fading_ribbons {
            r.advance(dt * 1000.0);
        }
        self.fading_ribbons.retain(|r| !r.is_empty());
    }

    /// A bolt gone (at `end` when it burst there): its trail left to fade.
    fn retire(&mut self, bolt: Bolt, end: Option<[f32; 3]>) {
        if let Some(mut trail) = bolt.trail {
            trail.dead = true;
            self.fading.push(trail);
        }
        if let Some(mut r) = bolt.ribbon {
            if let Some(end) = end {
                r.add_point(end);
            }
            r.dead = true;
            self.fading_ribbons.push(r);
        }
    }

    /// The bolts' objects as [`ArenaScene::update_bolts`] left them, each turned along its flight and about it.
    fn bolt_objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
    ) -> Vec<DynamicObject> {
        let mut out = Vec::new();
        let bolts: Vec<(u32, u32, BoltPose)> = self
            .bolts
            .iter()
            .map(|(&i, b)| (i, b.db, (b.drawn(), b.velocity, b.age, b.size())))
            .collect();
        for (index, db, (at, v, age, size)) in bolts {
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
            let mut m = Mat::mul(&Mat::mul(&Mat::euler_z(yaw), &Mat::euler_x(pitch)), &spin);
            for r in 0..3 {
                for c in 0..3 {
                    m.m[r * 4 + c] *= size;
                }
            }
            m.set_column3(at);
            out.push(view.shape_object(assets, &shape, &instance, &m, object_id(index, 2)));
        }
        self.bolts_drawn = out.len();
        out
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

    /// The other figures' arms as the last frame posed them: whether each plays a death, and the action its arm thread plays
    /// (none: the thread is stopped).
    pub fn others_arms(&self) -> Vec<(bool, Option<String>)> {
        self.figures
            .values()
            .filter(|f| !f.own)
            .map(|f| {
                let arm = f
                    .anim
                    .arm_action()
                    .filter(|_| f.anim.arm_running())
                    .and_then(|a| f.anim.actions().get(a))
                    .map(|a| String::from_utf8_lossy(&a.name).into_owned());
                (f.anim.dying(), arm)
            })
            .collect()
    }

    /// The other figures' positions (feet) and yaws as the last frame drew them.
    pub fn others(&self) -> Vec<([f32; 3], f32)> {
        self.figures
            .values()
            .filter(|f| !f.own)
            .map(|f| (f.position, f.yaw))
            .collect()
    }

    /// The figures moved on by `dt` seconds to `world`'s Player records, with nothing drawn: the own one where the client predicts
    /// it, the others where the client runs them (`ghost_render` and `ghosts`); each animated and its held image posed, and the own
    /// view set.
    pub fn update_figures(
        &mut self,
        assets: &KitAssets,
        world: &TgeWorld,
        ghosts: &GhostPlayers,
        own: Option<OwnLook>,
        dt: f32,
    ) {
        self.own = None;
        let mut seen = Vec::new();
        let (mut shots, mut live, mut done) = (0, self.live, Vec::new());
        let mut rng = self.rng.take().unwrap_or_else(|| Rng::new(0x5eed));
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
                        position,
                        yaw,
                        pitch: head,
                        own: false,
                        instance,
                        anim,
                        held: None,
                        held_db: None,
                        aim: 0.0,
                    })
                }
            };
            figure.own = look.is_some();
            figure.aim = if figure.own { aim_ease(self.aim) } else { 0.0 };
            match look {
                Some(o) => {
                    figure.position = o.position.unwrap_or(position);
                    figure.yaw = o.yaw;
                    figure.pitch = o.pitch;
                    if let Some(c) = world.control_state.as_ref() {
                        motion.velocity = c.velocity;
                        motion.falling = c.falling;
                    }
                }
                None => {
                    (figure.position, figure.yaw) = match world.ghost_render.get(&index) {
                        Some(Pose::Euler { position, yaw, .. }) => (*position, *yaw),
                        _ => (position, yaw),
                    };
                    figure.pitch = head;
                    if let Some(m) = ghosts.get(index) {
                        figure.pitch = m.head() * max_look;
                        motion.velocity = m.velocity();
                    }
                }
            }
            motion.yaw = figure.yaw;
            // The held image.
            if image.map(|i| i.0) != figure.held_db {
                figure.held_db = image.map(|i| i.0);
                figure.held = held_shape.and_then(|(shape, offset)| Held::new(shape, offset));
            }
            // A held pistol is held up (`hold`), or at the eye while the server says its figure aims (`aim`).
            let named = |name: &[u8]| figure.anim.actions().iter().position(|a| a.name == name);
            let aiming = named(b"aim").filter(|a| motion.arm_action == u32::try_from(*a).ok());
            let hold = figure
                .held
                .as_ref()
                .and_then(|_| aiming.or_else(|| named(b"hold")));
            motion.arm_action = hold.and_then(|h| u32::try_from(h).ok());
            motion.look = ((figure.pitch / max_look + 1.0) * 0.5).clamp(0.0, 1.0);
            let s = figure.shape.shape();
            figure.anim.update(s, &mut figure.instance, &motion, dt);
            let detail = figure.shape.detail().unwrap_or(0);
            let _ = figure.instance.animate(s, detail, None, &mut |_| true);
            if let (Some(held), Some((db, fire_count))) = (figure.held.as_mut(), image) {
                if held.update(fire_count, dt) {
                    held.since_flare = 0.0;
                    shots += 1;
                    if let Some((e, seconds)) = fire_emitter(world, db) {
                        held.emitters.push((e, seconds * 1000.0));
                    }
                } else {
                    held.since_flare += dt;
                }
            }
            // The image's emitters follow the muzzle of the pistol the frame draws (the own figure's first-person one).
            let drawn = figure.held.as_ref().and_then(|h| {
                if figure.own {
                    Some(figure.view_image(h))
                } else {
                    figure.hand_image(h)
                }
            });
            if let Some(held) = figure.held.as_mut() {
                held.muzzle_at = drawn.map(|m| held.muzzle(&m));
                held.emit_ms += dt * 1000.0;
                let ms = held.emit_ms.floor().clamp(0.0, 1000.0) as u32;
                held.emit_ms = (held.emit_ms - ms as f32).clamp(0.0, 1.0);
                for (e, left) in &mut held.emitters {
                    e.advance(ms, &mut live);
                    if *left > 0.0
                        && ms > 0
                        && let Some((at, dir)) = held.muzzle_at
                    {
                        let run = (ms as f32).min(*left).ceil() as u32;
                        e.emit_path(at, dir, [0.0; 3], run, &mut rng, &mut live);
                        *left -= ms as f32;
                    }
                }
                for (mut e, _) in held.emitters.extract_if(.., |(_, left)| *left <= 0.0) {
                    e.dead = true;
                    done.push(e);
                }
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
        self.flashes += shots;
        self.live = live;
        self.rng = Some(rng);
        self.fading.extend(done);
    }

    /// The figures' objects as [`ArenaScene::update_figures`] left them: the own one and the pistol in its hand into the shadow
    /// maps only, and its first-person pistol at the eye.
    fn figure_objects(&mut self, view: &mut WorldView, assets: &KitAssets) -> Vec<DynamicObject> {
        let mut out = Vec::new();
        let mut holders = 0;
        for (&index, figure) in &mut self.figures {
            let body = figure.transform();
            let seen_bits = if figure.own { material::SHADOW_ONLY } else { 0 };
            let view_image = figure
                .held
                .as_ref()
                .filter(|_| figure.own)
                .map(|h| figure.view_image(h));
            let hand_image = figure.held.as_ref().and_then(|h| figure.hand_image(h));
            if let (Some(m), Some(held)) = (view_image, figure.held.as_ref()) {
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
            if let (Some(held), Some(m)) = (figure.held.as_ref(), hand_image) {
                let held_object =
                    view.shape_object(assets, &held.shape, &held.instance, &m, object_id(index, 1));
                out.push(marked(held_object, seen_bits));
                if !figure.own {
                    holders += 1;
                }
            }
        }
        self.holders_drawn = holders;
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
        let _ = dt;
        let mut seen = Vec::new();
        let mut out = self.figure_objects(view, assets);
        out.extend(self.bolt_objects(view, assets, world));
        for (&index, ghost) in &world.records.ghosts {
            let (Some(TgeClass::StaticShape), Some(_), Some(db)) =
                (ghost.class, ghost.record.as_ref(), ghost.datablock())
            else {
                continue;
            };
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
        self.cubes.retain(|i, _| seen.contains(i));
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
        let images = self
            .figures
            .values()
            .filter_map(|f| f.held.as_ref())
            .flat_map(|h| h.emitters.iter().map(|(e, _)| e));
        let trails = self
            .bolts
            .values()
            .filter_map(|b| b.trail.as_ref())
            .chain(&self.fading)
            .chain(images);
        let dist = |c: [f32; 3]| {
            (0..3)
                .map(|k| (c[k] - eye.position[k]).powi(2))
                .sum::<f32>()
                .sqrt()
        };
        for e in trails {
            let (Some(center), Some(d)) = (e.centre(), e.draw(right, up, &mut set, "trail")) else {
                continue;
            };
            self.trail_particles += e.particles.len();
            draws.push((dist(center), d));
        }
        self.ribbons_drawn = 0;
        let ribbons = self
            .bolts
            .values()
            .filter_map(|b| b.ribbon.as_ref())
            .chain(&self.fading_ribbons);
        for r in ribbons {
            let (Some(center), Some(d)) = (r.center(), r.draw(eye.position, &mut set, "ribbon"))
            else {
                continue;
            };
            self.ribbons_drawn += 1;
            draws.push((dist(center), d));
        }
        // The shots' flares at the muzzles the frame draws.
        for held in self.figures.values().filter_map(|f| f.held.as_ref()) {
            if let Some((at, dir)) = held.muzzle_at
                && held.since_flare < FLARE_SECONDS
            {
                let t = held.since_flare / FLARE_SECONDS;
                draws.extend(flare_draws(at, dir, t, eye.position, (right, up), &mut set));
            }
        }
        let decals = self.decals.draws(eye.position, &mut set);
        view.set_particles_and_decals(&draws, &decals, set);
    }

    /// `dt` seconds of the sights' blend toward the sights (`aiming`) or the hip; returns how far up they are (eased).
    pub fn update_aim(&mut self, aiming: bool, dt: f32) -> f32 {
        let step = dt.max(0.0) / AIM_SECONDS;
        self.aim = if aiming {
            (self.aim + step).min(1.0)
        } else {
            (self.aim - step).max(0.0)
        };
        aim_ease(self.aim)
    }

    /// How far the sights are up (eased, 0 to 1).
    pub fn aim(&self) -> f32 {
        aim_ease(self.aim)
    }

    /// The field of view now (degrees, horizontal): [`FOV`], narrowing to [`AIM_FOV`] as the sights come up.
    pub fn fov(&self) -> f32 {
        f32::from(FOV) + (AIM_FOV - f32::from(FOV)) * self.aim()
    }

    /// How much slower the mouse turns the view now: the narrower view's share of the full one (`tan` of the half angles).
    pub fn turn_scale(&self) -> f32 {
        let half = |deg: f32| (deg.to_radians() * 0.5).tan();
        half(self.fov()) / half(f32::from(FOV))
    }

    /// The eye: at the own figure's eye, else over the arena.
    pub fn eye(&self) -> Eye {
        match self.own {
            Some(o) => Eye {
                position: o.eye,
                pitch: o.pitch,
                yaw: o.yaw,
                fov: Some(self.fov().round() as u8),
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

/// The faces (as triangles) of the map's cubes (its StaticShapes, each the kit's unit cube scaled and turned as its record
/// says) whose bounds come within `reach` of `at`: the surface a decal there may lie on.
fn cube_triangles_near(world: &TgeWorld, at: [f32; 3], reach: f32) -> Vec<Triangle> {
    let mut out = Vec::new();
    for ghost in world.records.ghosts.values() {
        let (Some(TgeClass::StaticShape), Some(pose)) = (ghost.class, ghost.pose()) else {
            continue;
        };
        let Some((m, scale)) = pose_matrix(&pose, None) else {
            continue;
        };
        let half = scale.map(|s| s.abs() * 0.5);
        let radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
        let center = m.column(3);
        let far = (0..3)
            .map(|k| (center[k] - at[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        if far > radius + reach {
            continue;
        }
        let corner = |i: usize| {
            let local = [0, 1, 2].map(|k| if i >> k & 1 != 0 { 0.5 } else { -0.5 } * scale[k]);
            m.mul_p(local)
        };
        // Each face's corners counter-clockwise seen from outside (for a positive scale).
        for f in [
            [0, 4, 6, 2],
            [1, 3, 7, 5],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 2, 3, 1],
            [4, 5, 7, 6],
        ] {
            out.push([corner(f[0]), corner(f[1]), corner(f[2])]);
            out.push([corner(f[0]), corner(f[2]), corner(f[3])]);
        }
    }
    out
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
