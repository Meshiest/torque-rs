//! A client's ghosts drawn through the engine's moving shapes ([`torque_render_scene::moving`]), with what a live client adds
//! between the server's updates.
//!
//! - **Players** are posed live ([`Hooks`]): each frame their action, arm and head threads run as TGE's client runs them
//!   ([`PlayerAnimation`]) at the velocity they are drawn moving at (the client's own Player as it predicts it, the others where it
//!   runs them, [`GhostPlayers`]), with the arm action the server set (`setArmThread`) and the head's pitch; an image state's
//!   recoil plays its recoil sequence (`onImageRecoil`). The client's own
//!   Player in its first-person view casts only its shadow ([`GhostOptions::shadow_body`]), and the view is at its drawn `eye`
//!   node (`getRenderEyeTransform`).
//! - **Mounted images** are placed and posed by the engine ([`torque_render_scene::images`]): in the hand, or before the eye in the
//!   first person. A host's own motion of them ([`HeldMotion`]): raised to their sights while the client aims
//!   ([`GhostScene::update_aim`], [`Sights`]: the `sightRear` and `sightFront` nodes on the eye's line) and kicked by a shot
//!   ([`Kick`]). A shot (the slot's fire count changed) starts the fire state's `stateEmitter` at the muzzle, following it as the
//!   image moves, and flares there ([`ShotFlare`]).
//! - **Projectiles** fly on from their first record along its velocity (the client runs them on its own, as TGE's client runs a
//!   projectile ghost) until their explosion record hides them, drawn by the engine from
//!   [`ClientWorld::projectile_render`]. One whose source object the client holds is
//!   drawn leaving the source image's muzzle and curving onto its flight ([`BoltPath`]), growing out of it, turning about its
//!   flight. Its `particleEmitter` leaves a trail of particles, or a ribbon ([`GhostOptions::ribbon_trails`], [`Ribbon`]); its
//!   explosion on a Player bursts its explosion's particles there, on anything else leaves one of its decals on the surface
//!   the host gives ([`DecalManager`]).
//!
//! Credit: the image's state emitter and the hit burst follow `ShapeBase::updateImageAnimation` and `Explosion::explode` in
//! Torque3D's MIT-licensed `T3D/shapeImage.cpp` and `T3D/fx/explosion.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C)
//! 2015 Faust Logic, Inc.), the eye at the drawn `eye` node `Player::getRenderEyeTransform` in `T3D/player.cpp` (the same
//! copyrights). The code here is this crate's own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use crate::ghost_world::images::ImageRecoil;
use torque_render_scene::decals::{DecalManager, Triangle, projectile_decal};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::images::{HeldMotion, ImageClock, ImageDb, node_named, sight_offset};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::render_pose;
use torque_render_scene::moving::{
    Hooks, ImageFrames, LiveAsk, ShapeSelect, draws_culled, with_shadow_body,
};
use torque_render_scene::particles::{Emitter, Rng};
use torque_render_scene::ribbons::{Ribbon, projectile_ribbon};
use torque_render_scene::shapes::{DetailPolicy, load};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Draw, Filter, Vertex};

use super::view::WorldView;
use crate::client::conn::tge::TgeWorld;
use crate::client::player_anim::{PlayerAnimation, PlayerMotion, ServerAction};
use crate::client::predict::ghost_players::GhostPlayers;
use crate::ghost_world::tge::TgeClientSet;
use crate::ghost_world::world::{ClientWorld, Pose, ProjectileLook};
use crate::ghosts::{TgeClass, TgeRecord, Value};
use crate::shapes::instance::ShapeInstance;

/// The record world the scene draws.
type World = ClientWorld<TgeClientSet>;

/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;
/// The most a frame advances a Player's animation (seconds: a stall or a clock that jumped).
const MAX_FRAME_STEP: f32 = 0.25;

/// A shot's kick of the image that fired it (none in TGE: its `fire` sequence and its owner's recoil sequence alone): how long it
/// takes to reach its height and how fast it settles after (seconds), how far it goes at its height in the first person (back,
/// units; up, radians) and in the hand (up, radians; 0 leaves the hand's kick to the owner's recoil sequence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kick {
    pub rise: f32,
    pub settle: f32,
    pub back: f32,
    pub up: f32,
    pub up_hand: f32,
}

impl Kick {
    /// How far into its kick the image is `t` seconds after a shot: 0 at rest, 1 at its height (a quick rise, then a settle).
    pub fn at(&self, t: f32) -> f32 {
        if !(0.0..1.0).contains(&t) {
            0.0
        } else if t < self.rise {
            t / self.rise
        } else {
            (-(t - self.rise) / self.settle).exp()
        }
    }
}

/// Aiming down the sights: how long an image takes to come up to its sights (seconds), how far ahead of the eye its rear sight is
/// held ([`sight_offset`]), and where an image without sights is held there (its place in the eye's frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sights {
    pub seconds: f32,
    pub rear: f32,
    pub fallback: [f32; 3],
}

impl Default for Sights {
    fn default() -> Sights {
        Sights {
            seconds: 0.18,
            rear: 0.36,
            fallback: [0.0, 0.42, -0.057],
        }
    }
}

/// The ease of the sights' blend: `t` (0 to 1 of [`Sights::seconds`]) smoothed at both ends.
pub fn aim_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How a projectile is drawn leaving its source's muzzle: curving from there onto its flight over its first `blend` units, its
/// size growing from `start_size` of its own to all of it over its first `grow` units, and turning about its flight at `spin`
/// radians a second. The defaults draw it on its flight from the first frame, whole and unturned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoltPath {
    pub blend: f32,
    pub start_size: f32,
    pub grow: f32,
    pub spin: f32,
}

impl Default for BoltPath {
    fn default() -> BoltPath {
        BoltPath {
            blend: 0.0,
            start_size: 1.0,
            grow: 0.0,
            spin: 0.0,
        }
    }
}

/// A shot's flare at the muzzle: a star facing the eye (`texture`, `size` units wide head on), shrinking and fading over
/// `seconds`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotFlare {
    pub texture: String,
    pub seconds: f32,
    pub size: f32,
}

/// The bolt holes kept: at most `max`, each for `seconds`, fading out over its last `fade`; multiplied over the surface they
/// lie on (`Blend::Modulate2x`) with `multiply`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalLimits {
    pub max: usize,
    pub seconds: f32,
    pub fade: f32,
    pub multiply: bool,
}

impl Default for DecalLimits {
    fn default() -> DecalLimits {
        DecalLimits {
            max: 64,
            seconds: 25.0,
            fade: 4.0,
            multiply: false,
        }
    }
}

/// What a [`GhostScene`] draws beyond TGE's rules.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostOptions {
    /// Which ghosts the engine draws ([`ShapeSelect::Moving`]: Players, vehicles, items and projectiles, and static shapes that
    /// play a thread; a host that draws its static shapes itself keeps it).
    pub select: ShapeSelect,
    /// The client's own Player in its first-person view casts its shadow (else it is not drawn).
    pub shadow_body: bool,
    pub kick: Option<Kick>,
    pub sights: Sights,
    pub bolt: BoltPath,
    /// A projectile's trail is a ribbon ([`projectile_ribbon`]) instead of its emitter's particles.
    pub ribbon_trails: bool,
    pub flare: Option<ShotFlare>,
    pub decals: DecalLimits,
}

impl Default for GhostOptions {
    fn default() -> GhostOptions {
        GhostOptions {
            select: ShapeSelect::Moving,
            shadow_body: true,
            kick: None,
            sights: Sights::default(),
            bolt: BoltPath::default(),
            ribbon_trails: false,
            flare: None,
            decals: DecalLimits::default(),
        }
    }
}

/// The emitter an image's shot starts: the `stateEmitter` of its datablock's fire state (the state with `stateFire`), when
/// the client holds it with its particles, and how long it emits (`stateEmitterTime`, seconds). The datablock's record lists
/// each state's members in order (`state` first, then `fire`, ..., `emitter` and `emitterTime` when the state has one).
pub fn fire_emitter(world: &World, image: u32) -> Option<(Emitter, f32)> {
    let db = world.datablocks.get(&image)?;
    let mut firing = false;
    let mut found: Option<Emitter> = None;
    for (name, _, value) in &db.fields {
        match (name.as_str(), value) {
            ("state", _) if found.is_some() => break,
            ("state", _) => firing = false,
            ("fire", Value::Bool(fire)) => firing = *fire,
            ("emitter", Value::Id(Some(id))) if firing => {
                found = Some(Emitter::new(world, *id)?);
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

/// A shot's flare ([`ShotFlare`]) at the muzzle `at` along `dir`, `t` (0 to 1) through its time, seen from `eye` with the camera's `right` and
/// `up`: the star facing the eye at the muzzle.
fn flare_draws(
    flare: &ShotFlare,
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
    if let Some(tex) = set.get_modulated(&flare.texture) {
        let h = 0.5 * flare.size * (0.45 + 0.55 * fade);
        let c = [0, 1, 2].map(|k| at[k] + dir[k] * 0.01);
        let p = |sx: f32, sy: f32| [0, 1, 2].map(|k| c[k] + (right[k] * sx + up[k] * sy) * h);
        let corners = [p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)];
        // Drawn twice: the additive passes add up to a brighter flash than the texture alone.
        out.push((dist, sprite("flare star", corners, tex, fade)));
        out.push((dist, sprite("flare star", corners, tex, fade)));
    }
    out
}

/// A new trail for a bolt of ProjectileData `db`: its `particleEmitter`, when the client holds it as a `ParticleEmitterData`
/// with its particles.
pub fn bolt_trail(world: &World, db: u32) -> Option<Emitter> {
    let id = match world.datablocks.get(&db)?.get("particleEmitter", 0)? {
        Value::Id(id) => (*id)?,
        Value::Int(v) => u32::try_from(*v).ok()?,
        _ => return None,
    };
    Emitter::new(world, id)
}

/// The burst of a bolt of ProjectileData `db` hitting a figure at `at`: its `explosion`'s `particleEmitter`, `particleDensity`
/// particles within `particleRadius` (`Explosion::explode`), when the client holds them.
pub fn hit_burst(
    world: &World,
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
    let mut e = Emitter::new(world, id(explosion, "particleEmitter")?)?;
    let count = number(explosion, "particleDensity")
        .unwrap_or(10.0)
        .clamp(0.0, 256.0) as u32;
    let radius = number(explosion, "particleRadius").unwrap_or(0.0);
    e.emit_burst(at, normal, radius, count, rng, live);
    e.dead = true;
    Some(e)
}

/// A Player as the client animates it between frames: its posed instance, its animation, the node count of the shape they were
/// made for and the clock they were last run to (ms).
pub struct LivePlayer {
    instance: ShapeInstance,
    anim: PlayerAnimation,
    nodes: usize,
    at_ms: u32,
    /// The count of its images' recoils last played ([`ImageRecoil::count`]).
    recoils: Option<u32>,
}

/// What a frame's Player animation reads beyond its record: the velocity it is drawn moving at, its head's pitch (radians) and,
/// for the client's own, whether it falls; and its images' last recoil.
#[derive(Debug, Clone, Copy, Default)]
struct PlayerInput {
    motion: Option<([f32; 3], f32, Option<bool>)>,
    recoil: Option<ImageRecoil>,
}

/// The live poses and image clocks kept between frames, the frame's Player inputs and the host's motion of each held image.
#[derive(Default)]
struct GhostHooks {
    live: Mutex<HashMap<u32, LivePlayer>>,
    clocks: Mutex<HashMap<(u32, usize), ImageClock>>,
    inputs: HashMap<u32, PlayerInput>,
    motions: HashMap<(u32, usize), HeldMotion>,
}

impl Hooks<World> for GhostHooks {
    type Live = LivePlayer;

    fn take_live(&self, index: u32) -> Option<LivePlayer> {
        self.live.lock().ok()?.remove(&index)
    }

    fn take_clock(&self, ghost: u32, slot: usize) -> Option<ImageClock> {
        self.clocks.lock().ok()?.remove(&(ghost, slot))
    }

    fn held_motion(&self, ghost: u32, slot: usize) -> HeldMotion {
        self.motions
            .get(&(ghost, slot))
            .copied()
            .unwrap_or_default()
    }

    /// A Player's frame: its threads run by the time since its last frame ([`PlayerAnimation::update`]), then posed.
    fn advance_live(
        &self,
        world: &World,
        ask: LiveAsk<'_, World>,
        kept: Option<LivePlayer>,
        notes: &mut Vec<String>,
    ) -> (Option<LivePlayer>, Option<ShapeInstance>) {
        let _ = notes;
        let Some(record @ (TgeRecord::Player(_) | TgeRecord::AiPlayer(_))) =
            ask.ghost.record.as_ref()
        else {
            return (kept, None);
        };
        let kept = kept.filter(|l| l.nodes == ask.shape.nodes.len());
        let mut live = match kept {
            Some(l) => l,
            None => {
                let Ok(mut instance) = ShapeInstance::new(ask.shape, &mut |_| true) else {
                    return (None, None);
                };
                let Some(anim) = PlayerAnimation::new(ask.shape, &mut instance) else {
                    return (None, None);
                };
                LivePlayer {
                    instance,
                    anim,
                    nodes: ask.shape.nodes.len(),
                    at_ms: world.anim_ms,
                    recoils: self
                        .inputs
                        .get(&ask.index)
                        .and_then(|i| i.recoil)
                        .map(|r| r.count),
                }
            }
        };
        let dt = (world.anim_ms.wrapping_sub(live.at_ms) as f32 / 1000.0).min(MAX_FRAME_STEP);
        live.at_ms = world.anim_ms;
        let yaw = match render_pose(world, ask.index) {
            Some(Pose::Euler { yaw, .. }) => yaw,
            _ => 0.0,
        };
        let mut motion = PlayerMotion::of_record(record, yaw).unwrap_or(PlayerMotion {
            yaw,
            ..PlayerMotion::default()
        });
        motion.server_action = ServerAction::of_record(record);
        let input = self.inputs.get(&ask.index).copied().unwrap_or_default();
        if let Some((velocity, _, falling)) = input.motion {
            motion.velocity = velocity;
            if let Some(falling) = falling {
                motion.falling = falling;
            }
        }
        // A new recoil of one of its images (`onImageRecoil`) restarts its recoil thread.
        if let Some(r) = input.recoil
            && live.recoils != Some(r.count)
        {
            live.recoils = Some(r.count);
            live.anim.recoil(ask.shape, &mut live.instance, r.kind);
        }
        let head = input.motion.map_or(ask.pitch, |m| m.1);
        let max_look = max_look(world, ask.db);
        motion.look = ((head / max_look + 1.0) * 0.5).clamp(0.0, 1.0);
        live.anim.update(ask.shape, &mut live.instance, &motion, dt);
        let _ = live
            .instance
            .animate(ask.shape, ask.dl, None, &mut |_| true);
        let posed = live.instance.clone();
        (Some(live), Some(posed))
    }

    fn put_back(
        &self,
        index: u32,
        live: Option<LivePlayer>,
        clocks: Vec<((u32, usize), ImageClock)>,
    ) {
        if let (Some(l), Ok(mut kept)) = (live, self.live.lock()) {
            kept.insert(index, l);
        }
        if let Ok(mut kept) = self.clocks.lock() {
            kept.extend(clocks);
        }
    }
}

/// The `maxLookAngle` of PlayerData `db` (the angle a head fraction is of).
fn max_look(world: &World, db: u32) -> f32 {
    match world
        .datablocks
        .get(&db)
        .and_then(|d| d.get("maxLookAngle", 0))
    {
        Some(Value::F32(v)) if v.is_finite() && *v > 0.0 => *v,
        _ => DEFAULT_MAX_LOOK,
    }
}

/// A mounted image the scene follows: its datablock, the fire count last seen and the seconds since it changed (a shot), the
/// emitters its shots started (each with the milliseconds it still emits), the frame time they have not run yet (ms, under one),
/// and its muzzle's place and the barrel's direction as the frame draws it.
struct Held {
    datablock: u32,
    fire_count: u32,
    since_fire: f32,
    emitters: Vec<(Emitter, f32)>,
    emit_ms: f32,
    muzzle: Option<([f32; 3], [f32; 3])>,
}

/// A projectile the scene runs: where its first record put it, its velocity and age, and where it is drawn from at first less
/// where it is (its source's muzzle), taken back over [`BoltPath::blend`]; its trail.
struct Bolt {
    start: [f32; 3],
    velocity: [f32; 3],
    age: f32,
    offset: [f32; 3],
    trail: Option<Emitter>,
    ribbon: Option<Ribbon>,
}

impl Bolt {
    /// How far it has flown (units).
    fn flown(&self) -> f32 {
        self.velocity.iter().map(|v| v * v).sum::<f32>().sqrt() * self.age
    }

    /// Where it is drawn: on its flight, from the muzzle curving onto it.
    fn drawn(&self, path: &BoltPath) -> [f32; 3] {
        let k = if path.blend > 0.0 {
            (1.0 - (self.flown() / path.blend).clamp(0.0, 1.0)).powi(2)
        } else {
            0.0
        };
        [0, 1, 2].map(|i| self.start[i] + self.velocity[i] * self.age + self.offset[i] * k)
    }

    /// Its size as drawn (a share of its shape's): growing out of the muzzle.
    fn size(&self, path: &BoltPath) -> f32 {
        if path.grow <= 0.0 {
            return 1.0;
        }
        path.start_size + (1.0 - path.start_size) * (self.flown() / path.grow).clamp(0.0, 1.0)
    }

    /// Its render transform: turned along its flight and about it, sized, at its drawn place.
    fn transform(&self, path: &BoltPath) -> Mat {
        let v = self.velocity;
        let yaw = v[0].atan2(v[1]);
        let pitch = -v[2].atan2((v[0] * v[0] + v[1] * v[1]).sqrt());
        let spin = Mat::from_axis_angle([0.0, 1.0, 0.0], path.spin * self.age);
        let mut m = Mat::mul(&Mat::mul(&Mat::euler_z(yaw), &Mat::euler_x(pitch)), &spin);
        let size = self.size(path);
        for r in 0..3 {
            for c in 0..3 {
                m.m[r * 4 + c] *= size;
            }
        }
        m.set_column3(self.drawn(path));
        m
    }
}

/// The most hit bursts [`GhostScene::hit_bursts`] keeps.
pub const MAX_HIT_BURSTS: usize = 16;

/// The client's own Player as the last frame placed it: its ghost, its feet, its yaw and head's pitch (positive looks down), and
/// the eye.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnView {
    pub ghost: u32,
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub eye: [f32; 3],
}

/// The faces of `world` near a point, within a reach: the surface a projectile's decal there may lie on.
pub type Surfaces = dyn Fn(&World, [f32; 3], f32) -> Vec<Triangle>;

/// What a frame of [`GhostScene::update`] reads besides the world: the client's other Players as it runs them, the field of
/// view the eye is drawn with, the frame's time (seconds), and the faces near a point (within a reach) a projectile's decal may
/// lie on.
pub struct FrameInput<'a> {
    pub ghosts: Option<&'a GhostPlayers>,
    pub fov: Option<u8>,
    pub dt: f32,
    pub surfaces: &'a Surfaces,
}

/// A client's ghosts frame after frame (see the module docs).
pub struct GhostScene {
    pub options: GhostOptions,
    hooks: GhostHooks,
    frames: ImageFrames,
    held: BTreeMap<(u32, usize), Held>,
    bolts: BTreeMap<u32, Bolt>,
    /// The trails of the projectiles gone and the hits' bursts, fading out.
    fading: Vec<Emitter>,
    fading_ribbons: Vec<Ribbon>,
    /// The projectiles whose hit burst (each bursts once).
    burst: BTreeSet<u32>,
    /// Where the last hits on Players burst (the last [`MAX_HIT_BURSTS`]).
    pub hit_bursts: Vec<[f32; 3]>,
    /// The holes the projectiles left in the world.
    pub decals: DecalManager,
    rng: Rng,
    /// The live particles of every emitter.
    live: usize,
    /// The frame time the trails have not run yet (ms, under one).
    trail_ms: f32,
    /// How far the sights' blend has run (0 at the hip, 1 at the sights; not eased).
    aim: f32,
    /// Each image shape's sights ([`sight_offset`]) and its `sightFront` and `sightRear` nodes, by path.
    sights: HashMap<String, Option<SightNodes>>,
    own: Option<OwnView>,
    own_muzzle: Option<[f32; 3]>,
    own_sights: Option<[[f32; 3]; 2]>,
    /// The other Players as the last frame placed them: ghost, feet, yaw.
    others: Vec<(u32, [f32; 3], f32)>,
    /// The projectiles drawn in the last frame, and the other Players holding an image.
    pub bolts_drawn: usize,
    pub holders_drawn: usize,
    /// The trail particles and the ribbons drawn in the last frame.
    pub trail_particles: usize,
    pub ribbons_drawn: usize,
    /// The shots seen so far.
    pub flashes: usize,
}

/// An image shape's sights: where they are held ([`sight_offset`]) and their nodes (front, rear).
#[derive(Debug, Clone, Copy)]
struct SightNodes {
    place: Option<([f32; 3], f32)>,
    nodes: Option<[usize; 2]>,
}

impl GhostScene {
    /// A scene with nothing in it yet.
    pub fn new(options: GhostOptions) -> GhostScene {
        let d = options.decals;
        GhostScene {
            options,
            hooks: GhostHooks::default(),
            frames: ImageFrames::default(),
            held: BTreeMap::new(),
            bolts: BTreeMap::new(),
            fading: Vec::new(),
            fading_ribbons: Vec::new(),
            burst: BTreeSet::new(),
            hit_bursts: Vec::new(),
            decals: DecalManager::new(d.max, d.seconds, d.fade),
            rng: Rng::new(0x5eed),
            live: 0,
            trail_ms: 0.0,
            aim: 0.0,
            sights: HashMap::new(),
            own: None,
            own_muzzle: None,
            own_sights: None,
            others: Vec::new(),
            bolts_drawn: 0,
            holders_drawn: 0,
            trail_particles: 0,
            ribbons_drawn: 0,
            flashes: 0,
        }
    }

    /// Forgets every ghost (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        let options = self.options.clone();
        let flashes = self.flashes;
        *self = GhostScene::new(options);
        self.flashes = flashes;
    }

    /// `dt` seconds of the sights' blend toward the sights (`aiming`) or the hip; returns how far up they are (eased).
    pub fn update_aim(&mut self, aiming: bool, dt: f32) -> f32 {
        let step = dt.max(0.0) / self.options.sights.seconds.max(1e-3);
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

    /// The client's own Player as the last frame placed it.
    pub fn own(&self) -> Option<OwnView> {
        self.own
    }

    /// Where the own Player's first image's muzzle was drawn in the last frame.
    pub fn own_muzzle(&self) -> Option<[f32; 3]> {
        self.own_muzzle
    }

    /// Where the own Player's first image's sights (`sightFront`, `sightRear`) were drawn in the last frame.
    pub fn own_sights(&self) -> Option<[[f32; 3]; 2]> {
        self.own_sights
    }

    /// The other Players' feet and yaws as the last frame placed them, in ghost order.
    pub fn others(&self) -> Vec<([f32; 3], f32)> {
        self.others.iter().map(|o| (o.1, o.2)).collect()
    }

    /// The other Players' arms as the last frame posed them, in ghost order: whether each plays a death, and the action its arm
    /// thread plays (none: the thread is stopped).
    pub fn others_arms(&self) -> Vec<(bool, Option<String>)> {
        let Ok(live) = self.hooks.live.lock() else {
            return Vec::new();
        };
        self.others
            .iter()
            .filter_map(|o| live.get(&o.0))
            .map(|l| {
                let a = &l.anim;
                let arm = a
                    .arm_action()
                    .filter(|_| a.arm_running())
                    .and_then(|i| a.actions().get(i))
                    .map(|x| String::from_utf8_lossy(&x.name).into_owned());
                (a.dying(), arm)
            })
            .collect()
    }

    /// Where the projectiles are drawn this frame.
    pub fn bolt_positions(&self) -> Vec<[f32; 3]> {
        self.bolts
            .values()
            .map(|b| b.drawn(&self.options.bolt))
            .collect()
    }

    /// The frame's motion, with nothing drawn: the Players posed, the own view placed (the eye at its Player's drawn `eye` node,
    /// with `input.fov`), the images' shots, kicks and emitters, the projectiles moved on and put where the engine draws them,
    /// their trails, bursts and holes.
    pub fn update(&mut self, assets: &dyn Assets, world: &mut TgeWorld, input: &FrameInput<'_>) {
        let dt = input.dt.max(0.0);
        self.player_inputs(world, input.ghosts);
        self.place_players(assets, world, input.fov);
        self.update_images(assets, &world.records, dt);
        self.update_bolts(assets, &world.records, input, dt);
        let path = self.options.bolt;
        world
            .records
            .projectile_render
            .retain(|i, _| !self.bolts.contains_key(i));
        for (&index, b) in &self.bolts {
            let birth = world.records.birth_order.get(&index).copied().unwrap_or(0);
            world.records.projectile_render.insert(
                index,
                ProjectileLook {
                    birth,
                    transform: b.transform(&path).m,
                    velocity: b.velocity,
                    fade: 1.0,
                },
            );
        }
    }

    /// The frame's Player inputs: the own one's as the client predicts it, the others' as it runs them.
    fn player_inputs(&mut self, world: &TgeWorld, ghosts: Option<&GhostPlayers>) {
        self.hooks.inputs.clear();
        let control = world
            .control
            .object
            .filter(|_| world.control_render.is_some());
        for class in [TgeClass::Player, TgeClass::AiPlayer] {
            for (index, ghost) in world.of_class(class) {
                let motion = if Some(index) == control {
                    let head = match world.control_render {
                        Some(Pose::Euler { pitch, .. }) => pitch,
                        _ => 0.0,
                    };
                    world
                        .control_state
                        .as_ref()
                        .map(|c| (c.velocity, head, Some(c.falling)))
                } else {
                    let max = ghost
                        .datablock()
                        .map_or(DEFAULT_MAX_LOOK, |d| max_look(world, d));
                    ghosts
                        .and_then(|g| g.get(index))
                        .map(|m| (m.velocity(), m.head() * max, None))
                };
                let recoil = world
                    .mounted_images(index)
                    .into_iter()
                    .filter_map(|(_, m)| m.recoil)
                    .max_by_key(|r| r.count);
                self.hooks
                    .inputs
                    .insert(index, PlayerInput { motion, recoil });
            }
        }
    }

    /// Each Player posed for the frame (its live pose run), the others' places kept, and the own one's view: at its drawn `eye`
    /// node in its first-person view.
    fn place_players(&mut self, assets: &dyn Assets, world: &mut TgeWorld, fov: Option<u8>) {
        self.own = None;
        self.others.clear();
        let control = world
            .control
            .object
            .filter(|_| world.control_render.is_some());
        let mut players = Vec::new();
        for class in [TgeClass::Player, TgeClass::AiPlayer] {
            players.extend(world.of_class(class).map(|(i, _)| i));
        }
        players.sort_unstable();
        for index in players {
            let w: &World = &world.records;
            let eye = self
                .frames
                .owner_nodes(w, &self.hooks, assets, index, &[b"eye"])
                .and_then(|n| n.into_iter().next().flatten())
                .map(|m| m.column(3));
            let Some(Pose::Euler {
                position,
                yaw,
                pitch,
            }) = render_pose(w, index)
            else {
                continue;
            };
            if Some(index) != control {
                self.others.push((index, position, yaw));
                continue;
            }
            let eye = eye.unwrap_or_else(|| w.eye().map_or(position, |e| e.position));
            self.own = Some(OwnView {
                ghost: index,
                position,
                yaw,
                pitch,
                eye,
            });
            if world.hide_control {
                world.records.view = Some(Eye {
                    position: eye,
                    pitch,
                    yaw,
                    fov: fov.or(world.control.server_fov.or(world.control.client_fov)),
                });
            }
        }
    }

    /// The sights of the image shape at `path`, loaded once.
    fn sight_nodes(
        &mut self,
        assets: &dyn Assets,
        world: &World,
        path: &str,
    ) -> Option<SightNodes> {
        let rear = self.options.sights.rear;
        *self.sights.entry(path.to_string()).or_insert_with(|| {
            let mut notes = Vec::new();
            let loaded = load(
                &|p| world.constructor_sequences(p),
                assets,
                path,
                &mut notes,
            )?;
            let shape = &loaded.model.shape;
            let nodes = node_named(shape, b"sightFront").zip(node_named(shape, b"sightRear"));
            Some(SightNodes {
                place: sight_offset(shape, rear),
                nodes: nodes.map(|(f, r)| [f, r]),
            })
        })
    }

    /// The mounted images' frame: each shot (a new fire count) kicks the image, flares and starts the fire state's emitter; the
    /// host's motion of each (kicked, raised to the sights in the first person); the muzzles where the frame draws them and the
    /// emitters run there.
    fn update_images(&mut self, assets: &dyn Assets, world: &World, dt: f32) {
        let control = world
            .control
            .object
            .filter(|_| world.control_render.is_some());
        let first_person = world.hide_control;
        let mut seen = BTreeSet::new();
        let mut started = Vec::new();
        let mut ghosts: Vec<u32> = world.images.keys().copied().collect();
        ghosts.sort_unstable();
        for &ghost in &ghosts {
            for (slot, image) in world.mounted_images(ghost) {
                seen.insert((ghost, slot));
                let held = self.held.entry((ghost, slot)).or_insert(Held {
                    datablock: image.datablock,
                    fire_count: image.fire_count,
                    since_fire: f32::MAX,
                    emitters: Vec::new(),
                    emit_ms: 0.0,
                    muzzle: None,
                });
                if held.datablock != image.datablock {
                    held.datablock = image.datablock;
                    held.fire_count = image.fire_count;
                    held.since_fire = f32::MAX;
                }
                if held.fire_count != image.fire_count {
                    held.fire_count = image.fire_count;
                    held.since_fire = 0.0;
                    self.flashes += 1;
                    started.push((ghost, slot));
                } else {
                    held.since_fire = (held.since_fire + dt).min(f32::MAX);
                }
            }
        }
        for (ghost, slot) in started {
            let db = self.held[&(ghost, slot)].datablock;
            if let Some((e, seconds)) = fire_emitter(world, db)
                && let Some(h) = self.held.get_mut(&(ghost, slot))
            {
                h.emitters.push((e, seconds * 1000.0));
            }
        }
        let mut gone: Vec<(u32, usize)> = Vec::new();
        for key in self.held.keys() {
            if !seen.contains(key) {
                gone.push(*key);
            }
        }
        for key in gone {
            if let Some(h) = self.held.remove(&key) {
                for (mut e, _) in h.emitters {
                    e.dead = true;
                    self.fading.push(e);
                }
            }
        }
        // The host's motion of each image: its kick, and in the first person its rise to the sights.
        self.hooks.motions.clear();
        let aim = self.aim();
        let keys: Vec<(u32, usize)> = self.held.keys().copied().collect();
        for (ghost, slot) in keys {
            let h = &self.held[&(ghost, slot)];
            let kick = self.options.kick.map_or(0.0, |k| k.at(h.since_fire));
            let db = h.datablock;
            let mut motion = HeldMotion::default();
            if let Some(k) = self.options.kick.filter(|k| kick > 0.0 && k.up_hand != 0.0) {
                motion.hand = Some(Mat::euler_x(-k.up_hand * kick));
            }
            let own_eye = first_person && Some(ghost) == control;
            if own_eye
                && let Some(image) = ImageDb::of(world, db)
                && let Some(eye_offset) = image.eye_offset
                && (aim > 0.0 || kick > 0.0)
            {
                let sights = self.sight_nodes(assets, world, &image.shape_file);
                let fallback = (self.options.sights.fallback, 0.0);
                let (place, pitch) = sights.and_then(|s| s.place).unwrap_or(fallback);
                let (back, up) = self.options.kick.map_or((0.0, 0.0), |k| (k.back, k.up));
                let mut m = Mat::mul(
                    &eye_offset,
                    &Mat::mul(&Mat::euler_x(-up * kick), &Mat::euler_x(pitch * aim)),
                );
                let from = eye_offset.column(3);
                let [x, y, z] = [0, 1, 2].map(|k| from[k] + (place[k] - from[k]) * aim);
                m.set_column3([x, y - back * kick, z + 0.2 * back * kick]);
                motion.eye = Some(m);
            }
            if motion != HeldMotion::default() {
                self.hooks.motions.insert((ghost, slot), motion);
            }
        }
        // The muzzles where the frame draws them, and the emitters following them.
        let mut done = Vec::new();
        for ((ghost, slot), h) in &mut self.held {
            h.muzzle = self
                .frames
                .node_frame(world, &self.hooks, assets, *ghost, *slot, -1)
                .map(|(m, _)| (m.column(3), m.mul_v([0.0, 1.0, 0.0])));
            h.emit_ms += dt * 1000.0;
            let ms = h.emit_ms.floor().clamp(0.0, 1000.0) as u32;
            h.emit_ms = (h.emit_ms - ms as f32).clamp(0.0, 1.0);
            for (e, left) in &mut h.emitters {
                e.advance(ms, &mut self.live);
                if *left > 0.0
                    && ms > 0
                    && let Some((at, dir)) = h.muzzle
                {
                    let run = (ms as f32).min(*left).ceil() as u32;
                    e.emit_path(at, dir, [0.0; 3], run, &mut self.rng, &mut self.live);
                    *left -= ms as f32;
                }
            }
            for (mut e, _) in h.emitters.extract_if(.., |(_, left)| *left <= 0.0) {
                e.dead = true;
                done.push(e);
            }
        }
        self.fading.extend(done);
        // The own Player's first image: its muzzle and sights as drawn.
        self.own_muzzle = None;
        self.own_sights = None;
        if let Some(own) = self.own
            && let Some(((_, slot), h)) = self
                .held
                .range((own.ghost, 0)..=(own.ghost, usize::MAX))
                .next()
        {
            let (slot, muzzle) = (*slot, h.muzzle);
            self.own_muzzle = muzzle.map(|m| m.0);
            let nodes = ImageDb::of(world, h.datablock)
                .and_then(|image| self.sight_nodes(assets, world, &image.shape_file))
                .and_then(|s| s.nodes);
            if let Some(nodes) = nodes {
                let mut at = nodes.map(|n| {
                    self.frames
                        .node_frame(world, &self.hooks, assets, own.ghost, slot, n as i32)
                        .map(|(m, _)| m.column(3))
                });
                if let [Some(front), Some(rear)] = [at[0].take(), at[1].take()] {
                    self.own_sights = Some([front, rear]);
                }
            }
        }
    }

    /// The projectiles moved on by `dt` seconds to `world`'s Projectile records: a new one drawn from its source's muzzle, an
    /// exploded or gone one's trail left to fade, a hit's burst or hole made, the trails run.
    fn update_bolts(
        &mut self,
        assets: &dyn Assets,
        world: &World,
        input: &FrameInput<'_>,
        dt: f32,
    ) {
        let path = self.options.bolt;
        // The trails run whole milliseconds; the rest waits for the next frame.
        self.trail_ms += dt * 1000.0;
        let ms = self.trail_ms.floor().clamp(0.0, 1000.0) as u32;
        self.trail_ms = (self.trail_ms - ms as f32).clamp(0.0, 1.0);
        for e in &mut self.fading {
            e.advance(ms, &mut self.live);
        }
        self.decals.advance(dt);
        self.fading.retain(|e| !e.particles.is_empty());
        let mut seen = Vec::new();
        for (&index, ghost) in &world.ghosts {
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
                use crate::collide::types::{PLAYER, VEHICLE};
                if x.hit_type != 0 && self.burst.insert(index) {
                    let at = x.position.map(|v| v.get());
                    let normal = x.normal.map(|v| v.get());
                    if x.hit_type & PLAYER != 0 {
                        if let Some(e) =
                            hit_burst(world, db, at, normal, &mut self.rng, &mut self.live)
                        {
                            self.fading.push(e);
                            self.hit_bursts.push(at);
                            let cut = self.hit_bursts.len().saturating_sub(MAX_HIT_BURSTS);
                            self.hit_bursts.drain(..cut);
                        }
                    } else if x.hit_type & VEHICLE == 0
                        && let Some(mut d) = projectile_decal(world, db, &mut self.rng)
                    {
                        if self.options.decals.multiply {
                            // Multiplied: the hole takes the color, light and shadow of the surface it is on.
                            d.blend = torque_render_scene::Blend::Modulate2x;
                        }
                        let angle = self.rng.f() * std::f32::consts::TAU;
                        let reach = d.size[0].max(d.size[1]);
                        let surface = (input.surfaces)(world, at, reach);
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
                    v.value(crate::ghosts::projectile::VELOCITY_NORMAL_BITS)
                });
                // Drawn from its source's muzzle as the frame draws it.
                let muzzle = initial.source.as_ref().and_then(|s| {
                    self.frames
                        .node_frame(world, &self.hooks, assets, s.object, s.slot as usize, -1)
                        .map(|(m, _)| m.column(3))
                });
                let offset = match muzzle {
                    Some(m) if path.blend > 0.0 => [0, 1, 2].map(|k| m[k] - start[k]),
                    _ => [0.0; 3],
                };
                self.bolts.insert(
                    index,
                    Bolt {
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
                match projectile_ribbon(world, db).filter(|_| self.options.ribbon_trails) {
                    Some(data) => {
                        let mut r = Ribbon::new(data);
                        r.add_point(bolt.drawn(&path));
                        bolt.ribbon = Some(r);
                    }
                    None => bolt.trail = bolt_trail(world, db),
                }
            }
            let from = bolt.drawn(&path);
            bolt.age += dt;
            let (at, v) = (bolt.drawn(&path), bolt.velocity);
            if let Some(trail) = bolt.trail.as_mut()
                && ms > 0
            {
                trail.advance(ms, &mut self.live);
                trail.emit_trail(from, at, v, ms, &mut self.rng, &mut self.live);
            }
            if let Some(r) = bolt.ribbon.as_mut() {
                r.advance(dt * 1000.0);
                r.add_point(at);
            }
        }
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

    /// A projectile gone (at `end` when it burst there): its trail left to fade.
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

    /// The frame's objects of the ghosts the engine draws ([`GhostOptions::select`]) as [`GhostScene::update`] left them.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &dyn Assets,
        world: &World,
    ) -> Vec<DynamicObject> {
        let mut set = TextureSet::new(assets);
        let mut notes = Vec::new();
        let select = self.options.select;
        let hooks = &self.hooks;
        let shaped = with_shadow_body(self.options.shadow_body, || {
            draws_culled(
                world,
                hooks,
                assets,
                &mut set,
                &mut notes,
                DetailPolicy::Highest,
                select,
                None,
            )
        });
        let control = world.control.object;
        let drawn = |i: &u32| shaped.transforms.contains_key(i);
        self.bolts_drawn = self.bolts.keys().filter(|i| drawn(i)).count();
        self.holders_drawn = self
            .others
            .iter()
            .filter(|o| {
                Some(o.0) != control && drawn(&o.0) && !world.mounted_images(o.0).is_empty()
            })
            .count();
        view.ghost_objects(world, &shaped, set)
    }

    /// The frame's particles (the trails, the bursts, the images' emitters, the ribbons and the shots' flares, facing `eye`) and
    /// the projectiles' holes, into `view`.
    pub fn effects(&mut self, view: &mut WorldView, assets: &dyn Assets, eye: &Eye) {
        let camera = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
        let (right, up) = (camera.column(0), camera.column(2));
        let mut set = TextureSet::new(assets);
        let mut draws = Vec::new();
        self.trail_particles = 0;
        let images = self
            .held
            .values()
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
        if let Some(flare) = &self.options.flare {
            for h in self.held.values() {
                if let Some((at, dir)) = h.muzzle
                    && h.since_fire < flare.seconds
                {
                    let t = h.since_fire / flare.seconds;
                    draws.extend(flare_draws(
                        flare,
                        at,
                        dir,
                        t,
                        eye.position,
                        (right, up),
                        &mut set,
                    ));
                }
            }
        }
        let decals = self.decals.draws(eye.position, &mut set);
        view.set_particles_and_decals(&draws, &decals, set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kick_rises_quickly_and_settles() {
        let k = Kick {
            rise: 0.04,
            settle: 0.08,
            back: 0.06,
            up: 0.14,
            up_hand: 0.3,
        };
        assert_eq!(k.at(0.0), 0.0);
        assert_eq!(k.at(0.04), 1.0);
        assert!(k.at(0.02) > 0.4);
        assert!(k.at(0.08) > k.at(0.15) && k.at(0.3) < 0.05);
        assert_eq!(k.at(f32::MAX), 0.0);
    }

    #[test]
    fn a_bolt_leaves_the_muzzle_grows_and_curves_onto_its_flight() {
        let path = BoltPath {
            blend: 12.0,
            start_size: 0.3,
            grow: 1.5,
            spin: std::f32::consts::TAU,
        };
        let mut b = Bolt {
            start: [0.0; 3],
            velocity: [0.0, 100.0, 0.0],
            age: 0.0,
            offset: [0.2, 0.0, -0.1],
            trail: None,
            ribbon: None,
        };
        assert_eq!(b.drawn(&path), [0.2, 0.0, -0.1], "at the muzzle");
        assert!((b.size(&path) - 0.3).abs() < 1e-6);
        b.age = 0.05;
        let mid = b.drawn(&path);
        assert!(mid[0] > 0.0 && mid[0] < 0.2 && mid[1] == 5.0, "{mid:?}");
        assert_eq!(b.size(&path), 1.0);
        b.age = 0.2;
        assert_eq!(
            b.drawn(&path),
            [0.0, 20.0, 0.0],
            "on its flight past the curve"
        );
        // The defaults draw it on its flight, whole.
        let plain = BoltPath::default();
        b.age = 0.0;
        assert_eq!((b.drawn(&plain), b.size(&plain)), ([0.0; 3], 1.0));
        let m = b.transform(&plain);
        assert_eq!(m.column(3), [0.0; 3]);
        assert!(
            (m.mul_v([0.0, 1.0, 0.0])[1] - 1.0).abs() < 1e-6,
            "along its flight"
        );
    }

    #[test]
    fn the_sights_come_up_eased_and_go_down() {
        let mut g = GhostScene::new(GhostOptions::default());
        assert_eq!(g.update_aim(true, 0.09), aim_ease(0.5));
        assert_eq!(g.update_aim(true, 1.0), 1.0);
        assert_eq!(g.update_aim(false, 1.0), 0.0);
    }
}
