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
//!   first person, moved as the game's style asks ([`GhostStyle::held_motion`]). A shot (the slot's fire count changed) starts the
//!   fire state's `stateEmitter` at the muzzle, following it as the image moves.
//! - **Projectiles** fly on from their first record along its velocity (the client runs them on its own, as TGE's client runs a
//!   projectile ghost) until their explosion record hides them, drawn by the engine from
//!   [`ClientWorld::projectile_render`], where the game's style puts them ([`GhostStyle::projectile`]: on their flight by
//!   default). Its `particleEmitter` leaves a trail of particles, or a ribbon ([`GhostOptions::ribbon_trails`], [`Ribbon`]); its
//!   explosion on a Player bursts its explosion's particles there, on anything else leaves one of its decals on the surface
//!   the host gives ([`DecalManager`]).
//! - **A game's style** ([`GhostStyle`]): what a game adds to TGE's rules, such as a held image's kick or its rise to its
//!   sights, a projectile's drawn path out of the muzzle, or a shot's flare; [`Plain`] adds nothing.
//!
//! Credit: the image's state emitter and the hit burst follow `ShapeBase::updateImageAnimation` and `Explosion::explode` in
//! Torque3D's MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012 GarageGames, LLC) and `T3D/fx/explosion.cpp` (Copyright
//! (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), the eye at the drawn `eye` node
//! `Player::getRenderEyeTransform` in `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.). The code here is this crate's own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use torque_ghost_world::images::ImageRecoil;
use torque_render_scene::decals::{DecalManager, Triangle, projectile_decal};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::images::{HeldMotion, ImageClock};
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::render_pose;
use torque_render_scene::moving::{
    Hooks, ImageFrames, LiveAsk, ShapeSelect, draws_culled, with_shadow_body,
};
use torque_render_scene::particles::{Emitter, Rng};
use torque_render_scene::ribbons::{Ribbon, projectile_ribbon};
use torque_render_scene::shapes::DetailPolicy;
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::tracks::{TireTracks, TrackRules, TrackWheel};
use torque_render_scene::vehicle_dust::{VehicleDust, ground_under, running_vehicles};
use torque_render_scene::{Assets, Draw};

use super::view::WorldView;
use crate::camera_blend::teleported;
use crate::interiors::Interiors;
use torque_client::conn::tge::TgeWorld;
use torque_client::player_anim::{
    PlayerAnimation, PlayerMotion, SCRIPT_SLOTS, ScriptPlay, ServerAction, record_pose,
};
use torque_client::predict::ghost_players::GhostPlayers;
use torque_game::sim::player::poses::{Pose as BodyPose, PoseData, pose_blend};
use torque_ghost_world::tge::TgeClientSet;
use torque_ghost_world::threads::ThreadState;
use torque_ghost_world::world::{ClientWorld, Pose, ProjectileLook};
use torque_ghosts::{TgeClass, TgeRecord, Value};
use torque_shapes::instance::FLAG_CYCLIC;
use torque_shapes::instance::{ShapeInstance, ThreadId, find_sequence};

/// The record world the scene draws.
type World = ClientWorld<TgeClientSet>;

/// The look angle a head fraction is of when the datablock names none (radians).
pub const DEFAULT_MAX_LOOK: f32 = 1.4;
/// The most a frame advances a Player's animation (seconds: a stall or a clock that jumped).
const MAX_FRAME_STEP: f32 = 0.25;

/// The own Player's steady eye between poses: after a change of pose the view eases from where it stood to the new pose's eye
/// over the body's blend ([`pose_blend`]), what is left of the way kept in the Player's own frame so a turn meanwhile carries it.
/// A new own Player or a teleport ([`EyePose::cut`]) ends the ease: the view is at the new place at once.
#[derive(Debug, Clone, Copy, Default)]
struct EyePose {
    pose: BodyPose,
    /// The way left at the change (the old eye less the new one, the Player's frame), the seconds since and the blend's.
    left: [f32; 3],
    since: f32,
    seconds: f32,
    /// The eye of the last frame (the world).
    last: Option<[f32; 3]>,
    /// The own Player's ghost and feet the last frame.
    at: Option<(u32, [f32; 3])>,
}

impl EyePose {
    /// Whether the own Player is another than the last frame's, or teleported from where it stood then (`dt` seconds ago,
    /// [`teleported`]); either forgets the ease and the eye last drawn.
    fn cut(&mut self, ghost: u32, position: [f32; 3], dt: f32) -> bool {
        let cut = self
            .at
            .replace((ghost, position))
            .is_some_and(|(g, from)| g != ghost || teleported(from, position, dt));
        if cut {
            self.left = [0.0; 3];
            self.seconds = 0.0;
            self.last = None;
        }
        cut
    }

    /// The eye this frame for pose `pose`, whose own eye is at `target` (the world), the Player at yaw `yaw`, `dt` seconds on.
    fn step(&mut self, pose: BodyPose, target: [f32; 3], yaw: f32, dt: f32) -> [f32; 3] {
        let (s, c) = yaw.sin_cos();
        if pose != self.pose {
            if let (Some(seconds), Some(last)) = (pose_blend(self.pose, pose), self.last) {
                let d = [0, 1, 2].map(|k| last[k] - target[k]);
                self.left = [d[0] * c - d[1] * s, d[0] * s + d[1] * c, d[2]];
                self.seconds = seconds;
            } else {
                self.left = [0.0; 3];
                self.seconds = 0.0;
            }
            self.pose = pose;
            self.since = 0.0;
        }
        self.since += dt;
        let w = if self.seconds > 0.0 {
            let u = (self.since / self.seconds).clamp(0.0, 1.0);
            1.0 - u * u * (3.0 - 2.0 * u)
        } else {
            0.0
        };
        let [x, y, z] = self.left.map(|v| v * w);
        let eye = [
            target[0] + x * c + y * s,
            target[1] - x * s + y * c,
            target[2] + z,
        ];
        self.last = Some(eye);
        eye
    }
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
    /// A projectile's trail is a ribbon ([`projectile_ribbon`]) instead of its emitter's particles.
    pub ribbon_trails: bool,
    pub decals: DecalLimits,
    /// Players are posed by the synced rules of a server whose Players have hitboxes
    /// ([`PlayerAnimation::set_synced`]), so the figure drawn is the one hits are cast at.
    pub synced_poses: bool,
    /// A steady first-person view (the engine's own; TGE stands the view at the drawn `eye` node, so the view bobs with the
    /// walk and the run): the own Player's view stands at its `eye` node at the shape's rest pose, carried by the Player's real
    /// motion (its position, a landing, a mount), while the body drawn and its hitboxes keep their animation. The first
    /// person's images ride the view, so they stay steady with it.
    pub steady_eye: bool,
    /// With [`Self::steady_eye`], the view keeps the drawn eye's sway across the ground (its height steady); off, the view is
    /// steady every way.
    pub eye_sway: bool,
    /// A projectile stops where its line meets the world's surfaces as the client has them, waiting there for the server's
    /// explosion, instead of flying on through them until it comes (the line is the server's: its first update's raw floats).
    /// For a game whose client has the world's surfaces as the server collides with them.
    pub bolt_stops: bool,
    /// The poses a server's Players have, whose triggers their records show the pose with (the engine's own poses after
    /// Torque3D's, `::sim::player::pose`): the other Players are drawn crouched, prone or sprinting as their server chose. None:
    /// every other Player stands (TGE's). The own Player's pose is the one the client predicts either way, and with
    /// [`Self::steady_eye`] the view stands at the eye of its pose (its shape's `crouch_root` or `prone_root` held at its
    /// start), eased over the body's own blend between poses.
    pub poses: Option<PoseData>,
}

impl Default for GhostOptions {
    fn default() -> GhostOptions {
        GhostOptions {
            select: ShapeSelect::Moving,
            shadow_body: true,
            ribbon_trails: false,
            decals: DecalLimits::default(),
            synced_poses: false,
            steady_eye: false,
            eye_sway: false,
            bolt_stops: false,
            poses: None,
        }
    }
}

/// What a game adds to how a [`GhostScene`] draws its ghosts, beyond TGE's rules. Each method's default adds nothing, as
/// [`Plain`] draws them.
pub trait GhostStyle {
    /// Forgets what it keeps of the ghosts ([`GhostScene::clear`]: a new connection's ghosts are new).
    fn clear(&mut self) {}

    /// The host's motion of a held image this frame (e.g. kicked by a shot, raised to its sights), asked of every image each
    /// frame; none leaves it where its datablock places it.
    fn held_motion(&mut self, image: &HeldImage<'_>) -> HeldMotion {
        let _ = image;
        HeldMotion::default()
    }

    /// The client's own Player's first image once the frame placed it (none: it holds none), each frame: `node` gives where one
    /// of the image shape's nodes is drawn.
    fn own_image(
        &mut self,
        image: Option<&HeldImage<'_>>,
        node: &mut dyn FnMut(usize) -> Option<[f32; 3]>,
    ) {
        let _ = (image, node);
    }

    /// A projectile's render transform ([`Flight::transform`]: on its flight).
    fn projectile(&self, flight: &Flight) -> Mat {
        flight.transform()
    }

    /// An interior's draw of `material` (its triangles in the world, its texture the material's) before it joins the view
    /// ([`crate::interiors`]): a host may tint it or change its sampler. Asked when an interior's mesh is made.
    fn interior_draw(&mut self, material: &str, draw: &mut Draw) {
        let _ = (material, draw);
    }

    /// Draws added to the frame's particles, each with its distance from the eye (e.g. a shot's flare at a muzzle).
    fn effects(&mut self, frame: &EffectsFrame<'_>, set: &mut TextureSet) -> Vec<(f32, Draw)> {
        let _ = (frame, set);
        Vec::new()
    }
}

/// The style that adds nothing: TGE's rules alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plain;

impl GhostStyle for Plain {}

/// A mounted image as the frame holds it: its owner's ghost, its slot and datablock, the seconds since its last shot (a new fire
/// count; `f32::MAX` before one) and whether it is the client's own Player's before the eye in the first person.
pub struct HeldImage<'a> {
    pub world: &'a ClientWorld<TgeClientSet>,
    pub assets: &'a dyn Assets,
    pub ghost: u32,
    pub slot: usize,
    pub datablock: u32,
    pub since_fire: f32,
    pub own_eye: bool,
}

/// A projectile as the client runs it: where its first record put it, its velocity, its age (seconds) and where its source
/// image's muzzle was drawn when it appeared (when the client holds the source).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub start: [f32; 3],
    pub velocity: [f32; 3],
    pub age: f32,
    pub muzzle: Option<[f32; 3]>,
}

impl Flight {
    /// How far it has flown (units).
    pub fn flown(&self) -> f32 {
        self.velocity.iter().map(|v| v * v).sum::<f32>().sqrt() * self.age
    }

    /// Where its flight has it: its first record's place moved on by its velocity.
    pub fn position(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| self.start[i] + self.velocity[i] * self.age)
    }

    /// Its turn along its flight (yaw, then pitch), at the origin.
    pub fn heading(&self) -> Mat {
        let v = self.velocity;
        let yaw = v[0].atan2(v[1]);
        let pitch = -v[2].atan2((v[0] * v[0] + v[1] * v[1]).sqrt());
        Mat::mul(&Mat::euler_z(yaw), &Mat::euler_x(pitch))
    }

    /// Its render transform: turned along its flight, at [`Flight::position`].
    pub fn transform(&self) -> Mat {
        let mut m = self.heading();
        m.set_column3(self.position());
        m
    }
}

/// A held image's muzzle as the frame draws it (its place and the barrel's direction) and the seconds since its last shot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Muzzle {
    pub at: [f32; 3],
    pub dir: [f32; 3],
    pub since_fire: f32,
}

/// What [`GhostStyle::effects`] draws for: the eye, the camera's right and up, and the held images' muzzles.
pub struct EffectsFrame<'a> {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub muzzles: &'a [Muzzle],
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
/// made for, the clock they were last run to (ms) and the ghost's birth ([`ClientWorld::birth_order`]).
pub struct LivePlayer {
    instance: ShapeInstance,
    anim: PlayerAnimation,
    nodes: usize,
    at_ms: u32,
    /// A new object at the same ghost index (a respawned Player) starts a new animation at `root`.
    birth: Option<u64>,
    /// The count of its images' recoils last played ([`ImageRecoil::count`]).
    recoils: Option<u32>,
    /// Its `steering` thread while it drives a WheeledVehicle ([`GhostHooks::advance_live`]).
    steering: Option<ThreadId>,
}

/// A Player at the wheel (mount node 0) of a WheeledVehicle that plays its `steering` thread plays its own `steering` sequence,
/// when its shape has one, at the same place (engine addition: TGE turns the vehicle's thread only; a game's figure keyframes
/// its hands on the turning wheel there). Off the wheel the thread goes.
fn drive_steering(
    world: &World,
    index: u32,
    shape: &torque_shapes::shape::Shape,
    live: &mut LivePlayer,
) {
    let place = world
        .mount_of(index)
        .filter(|&(_, node)| node == 0)
        .and_then(|(on, _)| torque_render_scene::world::steering_place(world, on));
    let seq = find_sequence(shape, b"steering").and_then(|s| i32::try_from(s).ok());
    match (place, seq) {
        (Some(place), Some(seq)) => {
            if live.steering.is_none() {
                live.steering = live.instance.create_thread(shape);
                if let Some(t) = live.steering {
                    live.instance.set_sequence(shape, t, seq, place);
                }
            }
            if let Some(t) = live.steering {
                let at = live.instance.thread(t).map_or(place, |th| th.pos);
                live.instance.advance_pos(shape, t, place - at);
            }
        }
        _ => {
            if let Some(t) = live.steering.take() {
                live.instance.destroy_thread(shape, t);
            }
        }
    }
}

/// What a frame's Player animation reads beyond its record: the velocity it is drawn moving at, its head's pitch (radians) and,
/// for the client's own, whether it falls; and its images' last recoil.
#[derive(Debug, Clone, Copy, Default)]
struct PlayerInput {
    motion: Option<([f32; 3], f32, Option<bool>)>,
    recoil: Option<ImageRecoil>,
    /// Its pose: the own one's as the client predicts it, another's as its record shows it.
    pose: BodyPose,
}

/// The script threads of ghost `index` as a Player's animation plays them over its own ([`PlayerAnimation::script_threads`]):
/// each slot's sequence at its place at the world's clock (a thread that does not cycle held at its end; a stopped one at
/// its start).
fn script_plays(
    world: &World,
    index: u32,
    shape: &torque_shapes::shape::Shape,
) -> [Option<ScriptPlay>; SCRIPT_SLOTS] {
    let mut out = [None; SCRIPT_SLOTS];
    let Some(slots) = world.threads.get(&index) else {
        return out;
    };
    for (o, t) in out.iter_mut().zip(slots.iter()) {
        let Some(t) = t else {
            continue;
        };
        let Some(seq) = shape.sequences.get(t.sequence as usize) else {
            continue;
        };
        let pos = match t.state {
            ThreadState::Stop => 0.0,
            _ if t.at_end => {
                if t.forward {
                    1.0
                } else {
                    0.0
                }
            }
            _ => {
                let at = t.position(world.clock_ms) / seq.duration.max(1e-3);
                if seq.flags & FLAG_CYCLIC != 0 {
                    at.rem_euclid(1.0)
                } else {
                    at.clamp(0.0, 1.0)
                }
            }
        };
        *o = Some(ScriptPlay {
            sequence: t.sequence as usize,
            pos,
        });
    }
    out
}

/// The live poses and image clocks kept between frames, the frame's Player inputs and the host's motion of each held image.
#[derive(Default)]
struct GhostHooks {
    live: Mutex<HashMap<u32, LivePlayer>>,
    clocks: Mutex<HashMap<(u32, usize), ImageClock>>,
    inputs: HashMap<u32, PlayerInput>,
    motions: HashMap<(u32, usize), HeldMotion>,
    /// [`GhostOptions::synced_poses`].
    synced: bool,
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
        let birth = world.birth_order.get(&ask.index).copied();
        let kept = kept.filter(|l| l.nodes == ask.shape.nodes.len() && l.birth == birth);
        let mut live = match kept {
            Some(l) => l,
            None => {
                let Ok(mut instance) = ShapeInstance::new(ask.shape, &mut |_| true) else {
                    return (None, None);
                };
                let Some(mut anim) = PlayerAnimation::new(ask.shape, &mut instance) else {
                    return (None, None);
                };
                anim.set_synced(self.synced);
                LivePlayer {
                    instance,
                    anim,
                    nodes: ask.shape.nodes.len(),
                    at_ms: world.anim_ms,
                    birth,
                    recoils: self
                        .inputs
                        .get(&ask.index)
                        .and_then(|i| i.recoil)
                        .map(|r| r.count),
                    steering: None,
                }
            }
        };
        let dt = (world.anim_ms.wrapping_sub(live.at_ms) as f32 / 1000.0).min(MAX_FRAME_STEP);
        live.at_ms = world.anim_ms;
        let (yaw, position) = match render_pose(world, ask.index) {
            Some(Pose::Euler { yaw, position, .. }) => (yaw, Some(position)),
            _ => (0.0, None),
        };
        let mut motion = PlayerMotion::of_record(record, yaw).unwrap_or(PlayerMotion {
            yaw,
            ..PlayerMotion::default()
        });
        motion.server_action = ServerAction::of_record(record);
        motion.position = position;
        motion.mounted = world.mount_of(ask.index).is_some();
        let input = self.inputs.get(&ask.index).copied().unwrap_or_default();
        motion.pose = input.pose;
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
        // The script threads its server plays on it (a hit's flinch), over its own threads, as its hitboxes have them.
        let plays = script_plays(world, ask.index, ask.shape);
        live.anim
            .script_threads(ask.shape, &mut live.instance, &plays);
        drive_steering(world, ask.index, ask.shape, &mut live);
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

/// A projectile the scene runs: its flight and its trail.
struct Bolt {
    flight: Flight,
    trail: Option<Emitter>,
    ribbon: Option<Ribbon>,
    /// Where its flight met the world's surfaces as the client has them ([`first_hit`]): it waits there for the server's
    /// explosion instead of flying on through the wall.
    stop: Option<[f32; 3]>,
}

/// Where the segment `from`..`to` first crosses one of `faces` (either side), as its share of the way (0 to 1).
pub fn first_hit(from: [f32; 3], to: [f32; 3], faces: &[Triangle]) -> Option<f32> {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let d = sub(to, from);
    let mut best: Option<f32> = None;
    for [a, b, c] in faces {
        let (e1, e2) = (sub(*b, *a), sub(*c, *a));
        let h = cross(d, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-9 {
            continue;
        }
        let inv = 1.0 / det;
        let s = sub(from, *a);
        let u = dot(s, h) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(d, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = dot(e2, q) * inv;
        if (0.0..=1.0).contains(&t) && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    }
    best
}

/// The most hit bursts [`GhostScene::hit_bursts`] keeps.
pub const MAX_HIT_BURSTS: usize = 16;

/// The client's own Player as the last frame placed it: its ghost, its feet, its yaw and head's pitch (positive looks down), the
/// eye its first-person view stands at (steady with [`GhostOptions::steady_eye`]), and its `eye` node as the body is drawn
/// (TGE's eye, moving with the animation); and whether the frame cut to it: another Player than the last frame's, or this one
/// teleported ([`teleported`]), when the game's camera cuts too (a [`crate::camera_blend::CameraBlend`] and boom reset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnView {
    pub ghost: u32,
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub eye: [f32; 3],
    pub drawn_eye: [f32; 3],
    pub cut: bool,
}

/// The faces of `world` near a point, within a reach: the surface a projectile's decal there may lie on.
pub type Surfaces<'a> = dyn Fn(&World, [f32; 3], f32) -> Vec<Triangle> + 'a;

/// What a frame of [`GhostScene::update`] reads besides the world: the client's other Players as it runs them, the field of
/// view the eye is drawn with, the frame's time (seconds), and the faces near a point (within a reach) a projectile's decal may
/// lie on.
pub struct FrameInput<'a> {
    pub ghosts: Option<&'a GhostPlayers>,
    pub fov: Option<u8>,
    pub dt: f32,
    pub surfaces: &'a Surfaces<'a>,
}

/// A client's ghosts frame after frame (see the module docs), with a game's style `S`.
pub struct GhostScene<S = Plain> {
    pub options: GhostOptions,
    pub style: S,
    hooks: GhostHooks,
    frames: ImageFrames,
    /// Each ghost's birth as the last frame saw it ([`ClientWorld::birth_order`]): a ghost gone or born again since drops what
    /// the scene kept of it ([`GhostScene::forget_reborn`]).
    births: BTreeMap<u32, u64>,
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
    own: Option<OwnView>,
    own_muzzle: Option<[f32; 3]>,
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
    /// The map's interiors.
    interiors: Interiors,
    /// The own Player's eye between poses ([`GhostOptions::poses`]).
    eye_pose: EyePose,
    /// The dust the vehicles the client runs kick up ([`VehicleDust`]: TGE's tire and lift-off dust).
    pub dust: VehicleDust,
    /// The tire tracks they leave, when the game asks for them ([`GhostScene::set_tire_tracks`]).
    pub tracks: Option<TireTracks>,
    /// The datablocks whose shapes and textures were loaded ([`GhostScene::preload`]).
    preloaded: BTreeSet<u32>,
}

impl GhostScene {
    /// A scene with nothing in it yet, in the plain style.
    pub fn new(options: GhostOptions) -> GhostScene {
        GhostScene::with_style(options, Plain)
    }
}

impl<S: GhostStyle> GhostScene<S> {
    /// A scene with nothing in it yet, in `style`.
    pub fn with_style(options: GhostOptions, style: S) -> GhostScene<S> {
        let (d, synced) = (options.decals, options.synced_poses);
        GhostScene {
            options,
            style,
            hooks: GhostHooks {
                synced,
                ..GhostHooks::default()
            },
            frames: ImageFrames::default(),
            births: BTreeMap::new(),
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
            own: None,
            own_muzzle: None,
            others: Vec::new(),
            bolts_drawn: 0,
            holders_drawn: 0,
            trail_particles: 0,
            ribbons_drawn: 0,
            flashes: 0,
            interiors: Interiors::default(),
            eye_pose: EyePose::default(),
            dust: VehicleDust::default(),
            tracks: None,
            preloaded: BTreeSet::new(),
        }
    }

    /// Lays tire tracks behind the vehicles' wheels as `rules` say (none: no tracks, TGE's).
    pub fn set_tire_tracks(&mut self, rules: Option<TrackRules>) {
        self.tracks = rules.map(TireTracks::new);
    }

    /// Forgets every ghost (a new connection's ghosts are new), and the style what it keeps of them; the shots seen so far stay.
    pub fn clear(&mut self) {
        self.style.clear();
        self.interiors.clear();
        let tracks = self
            .tracks
            .as_ref()
            .map(|t| TireTracks::new(t.rules.clone()));
        let dust = VehicleDust::new(self.dust.rules);
        GhostScene {
            options: _,
            style: _,
            hooks: self.hooks,
            frames: self.frames,
            births: self.births,
            held: self.held,
            bolts: self.bolts,
            fading: self.fading,
            fading_ribbons: self.fading_ribbons,
            burst: self.burst,
            hit_bursts: self.hit_bursts,
            decals: self.decals,
            rng: self.rng,
            live: self.live,
            trail_ms: self.trail_ms,
            own: self.own,
            own_muzzle: self.own_muzzle,
            others: self.others,
            bolts_drawn: self.bolts_drawn,
            holders_drawn: self.holders_drawn,
            trail_particles: self.trail_particles,
            ribbons_drawn: self.ribbons_drawn,
            flashes: _,
            interiors: _,
            eye_pose: self.eye_pose,
            dust: _,
            tracks: _,
            preloaded: self.preloaded,
        } = GhostScene::new(self.options.clone());
        self.tracks = tracks;
        self.dust = dust;
    }

    /// The client's own Player as the last frame placed it.
    pub fn own(&self) -> Option<OwnView> {
        self.own
    }

    /// Where the own Player's first image's muzzle was drawn in the last frame.
    pub fn own_muzzle(&self) -> Option<[f32; 3]> {
        self.own_muzzle
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

    /// The place of each Player's `steering` thread as the last frame posed it (the Players at a wheel), by ghost.
    pub fn player_steering(&self) -> Vec<(u32, f32)> {
        let Ok(live) = self.hooks.live.lock() else {
            return Vec::new();
        };
        let mut out: Vec<(u32, f32)> = live
            .iter()
            .filter_map(|(&g, l)| Some((g, l.instance.thread(l.steering?)?.pos)))
            .collect();
        out.sort_unstable_by_key(|a| a.0);
        out
    }

    /// The action each Player's action thread plays as the last frame posed it, by ghost, in ghost order.
    pub fn player_actions(&self) -> Vec<(u32, String)> {
        let Ok(live) = self.hooks.live.lock() else {
            return Vec::new();
        };
        let mut out: Vec<(u32, String)> = live
            .iter()
            .filter_map(|(&g, l)| {
                let (i, _) = l.anim.action();
                let a = l.anim.actions().get(i)?;
                Some((g, String::from_utf8_lossy(&a.name).into_owned()))
            })
            .collect();
        out.sort_unstable_by_key(|a| a.0);
        out
    }

    /// Whether each Player's arm and head threads run as the last frame posed it, by ghost, in ghost order.
    pub fn player_threads(&self) -> Vec<(u32, bool, bool)> {
        let Ok(live) = self.hooks.live.lock() else {
            return Vec::new();
        };
        let mut out: Vec<(u32, bool, bool)> = live
            .iter()
            .map(|(&g, l)| (g, l.anim.arm_running(), l.anim.head_running()))
            .collect();
        out.sort_unstable_by_key(|a| a.0);
        out
    }

    /// Where the projectiles stopped at the world's surfaces as this client has them (the ones that did).
    pub fn bolt_stops(&self) -> Vec<[f32; 3]> {
        self.bolts.values().filter_map(|b| b.stop).collect()
    }

    /// Where the projectiles are drawn this frame.
    pub fn bolt_positions(&self) -> Vec<[f32; 3]> {
        self.bolts
            .values()
            .map(|b| self.style.projectile(&b.flight).column(3))
            .collect()
    }

    /// The frame's motion, with nothing drawn: the Players posed, the own view placed (the eye at its Player's drawn `eye` node,
    /// with `input.fov`), the images' shots, motions and emitters, the projectiles moved on and put where the engine draws them,
    /// their trails, bursts and holes.
    pub fn update(&mut self, assets: &dyn Assets, world: &mut TgeWorld, input: &FrameInput<'_>) {
        let dt = input.dt.max(0.0);
        self.preload(assets, &world.records);
        self.forget_reborn(&world.records.birth_order);
        self.player_inputs(world, input.ghosts);
        self.place_players(assets, world, input.fov, dt);
        self.update_images(assets, &world.records, dt);
        self.update_bolts(assets, &world.records, input, dt);
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
                    transform: self.style.projectile(&b.flight).m,
                    velocity: b.flight.velocity,
                    fade: 1.0,
                },
            );
        }
        // The vehicles' dust and tracks, on the ground the host's surfaces give.
        let records = &world.records;
        let vehicles = running_vehicles(records);
        let surfaces = |at: [f32; 3], reach: f32| (input.surfaces)(records, at, reach);
        let ground = |at: [f32; 3], depth: f32| ground_under(&surfaces(at, 1.0), at, depth);
        self.dust.update(records, &vehicles, &ground, dt);
        if let Some(tracks) = self.tracks.as_mut() {
            let wheels: Vec<TrackWheel> = vehicles
                .iter()
                .flat_map(|v| {
                    v.look
                        .wheels
                        .iter()
                        .enumerate()
                        .filter(|(_, w)| w.fitted)
                        .map(|(k, w)| TrackWheel {
                            key: (v.index, k),
                            contact: w.contact,
                            point: w.point,
                        })
                })
                .collect();
            tracks.update(&wheels, &surfaces, dt);
        }
    }

    /// Loads the shapes and the particles' and decals' textures each new datablock names (`shapeFile`, `projectileShapeName`,
    /// `textureName`), as TGE's datablocks preload theirs: kept by the loaders' caches, they are not read and decoded mid-game
    /// in the frame of a first shot, hit or hole.
    fn preload(&mut self, assets: &dyn Assets, world: &World) {
        if world.datablocks.len() == self.preloaded.len() {
            return;
        }
        let mut set = TextureSet::new(assets);
        let mut notes = Vec::new();
        for (&id, db) in &world.datablocks {
            if !self.preloaded.insert(id) {
                continue;
            }
            for (name, _, value) in &db.fields {
                let Value::Str(text) = value else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(text);
                match name.as_str() {
                    "shapeFile" | "projectileShapeName" => {
                        let seqs = |p: &str| world.constructor_sequences(p);
                        let _ = torque_render_scene::shapes::load(&seqs, assets, &text, &mut notes);
                    }
                    "textureName" | "textureNames" => {
                        let _ = set.get_modulated(&text);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Drops what the scene keeps of each ghost gone or born again since the last frame (`births`: the world's), so an object that
    /// takes a ghost's index starts fresh: its live pose, its images' clocks and held state, its projectile flight and burst, its
    /// vehicle dust and tire-track strips.
    fn forget_reborn(&mut self, births: &BTreeMap<u32, u64>) {
        if let Ok(mut live) = self.hooks.live.lock() {
            live.retain(|i, l| l.birth.is_some() && births.get(i).copied() == l.birth);
        }
        let reborn: BTreeSet<u32> = self
            .births
            .iter()
            .filter(|&(i, b)| births.get(i) != Some(b))
            .map(|(&i, _)| i)
            .collect();
        self.births.clone_from(births);
        if reborn.is_empty() {
            return;
        }
        if let Ok(mut clocks) = self.hooks.clocks.lock() {
            clocks.retain(|(g, _), _| !reborn.contains(g));
        }
        let held: Vec<(u32, usize)> = (self.held.keys())
            .filter(|(g, _)| reborn.contains(g))
            .copied()
            .collect();
        for key in held {
            if let Some(h) = self.held.remove(&key) {
                for (mut e, _) in h.emitters {
                    e.dead = true;
                    self.fading.push(e);
                }
            }
        }
        for &i in &reborn {
            if let Some(bolt) = self.bolts.remove(&i) {
                self.retire(bolt, None);
            }
            self.burst.remove(&i);
            self.dust.forget(i);
            if let Some(t) = self.tracks.as_mut() {
                t.forget(i);
            }
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
                let pose = if Some(index) == control {
                    world
                        .control_state
                        .as_ref()
                        .map_or(BodyPose::Stand, |c| c.pose)
                } else {
                    self.options
                        .poses
                        .as_ref()
                        .zip(ghost.record.as_ref())
                        .and_then(|(d, r)| record_pose(r, d))
                        .unwrap_or(BodyPose::Stand)
                };
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
                self.hooks.inputs.insert(
                    index,
                    PlayerInput {
                        motion,
                        recoil,
                        pose,
                    },
                );
            }
        }
    }

    /// Each Player posed for the frame (its live pose run), the others' places kept, and the own one's view: at its drawn `eye`
    /// node in its first-person view.
    fn place_players(
        &mut self,
        assets: &dyn Assets,
        world: &mut TgeWorld,
        fov: Option<u8>,
        dt: f32,
    ) {
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
            let cut = self.eye_pose.cut(index, position, dt);
            // A steady view: the eye at the rest pose, where the Player really is (the engine's own, [`GhostOptions::steady_eye`]),
            // lowered with its pose and eased between poses as its body is.
            let pose = self
                .hooks
                .inputs
                .get(&index)
                .map_or(BodyPose::Stand, |i| i.pose);
            let sequence = pose.action_prefix().map(|n| format!("{n}_root"));
            let rest = self
                .options
                .steady_eye
                .then(|| {
                    self.frames
                        .owner_held_nodes(
                            w,
                            assets,
                            index,
                            &[b"eye"],
                            sequence.as_deref().map(str::as_bytes),
                        )
                        .and_then(|n| n.into_iter().next().flatten())
                        .map(|m| m.column(3))
                })
                .flatten()
                .map(|target| self.eye_pose.step(pose, target, yaw, dt));
            let fallback = || w.eye().map_or(position, |e| e.position);
            let drawn_eye = eye.unwrap_or_else(fallback);
            let eye = match (rest, eye) {
                (Some(r), Some(d)) if self.options.eye_sway => [d[0], d[1], r[2]],
                (Some(r), _) => r,
                (None, _) => drawn_eye,
            };
            self.own = Some(OwnView {
                ghost: index,
                position,
                yaw,
                pitch,
                eye,
                drawn_eye,
                cut,
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

    /// The mounted images' frame: each shot (a new fire count) starts the fire state's emitter; the style's motion of each; the
    /// muzzles where the frame draws them and the emitters run there; the style's look at the own Player's first image.
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
        // The style's motion of each image.
        self.hooks.motions.clear();
        for (&(ghost, slot), h) in &self.held {
            let image = HeldImage {
                world,
                assets,
                ghost,
                slot,
                datablock: h.datablock,
                since_fire: h.since_fire,
                own_eye: first_person && Some(ghost) == control,
            };
            let motion = self.style.held_motion(&image);
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
        // The own Player's first image: its muzzle as drawn, and the style's look at it.
        self.own_muzzle = None;
        let own = self.own.and_then(|own| {
            let ((_, slot), h) = self
                .held
                .range((own.ghost, 0)..=(own.ghost, usize::MAX))
                .next()?;
            Some((own.ghost, *slot, h.datablock, h.since_fire, h.muzzle))
        });
        let (frames, hooks) = (&mut self.frames, &self.hooks);
        match own {
            Some((ghost, slot, datablock, since_fire, muzzle)) => {
                self.own_muzzle = muzzle.map(|m| m.0);
                let image = HeldImage {
                    world,
                    assets,
                    ghost,
                    slot,
                    datablock,
                    since_fire,
                    own_eye: first_person && Some(ghost) == control,
                };
                let mut node = |n: usize| {
                    frames
                        .node_frame(world, hooks, assets, ghost, slot, n as i32)
                        .map(|(m, _)| m.column(3))
                };
                self.style.own_image(Some(&image), &mut node);
            }
            None => self.style.own_image(None, &mut |_| None),
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
                use torque_collide::types::{PLAYER, VEHICLE};
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
                        let mut surface = (input.surfaces)(world, at, reach);
                        surface.extend(self.interiors.faces_near(at, reach));
                        self.decals.add_clipped(&d, at, normal, angle, &surface);
                    }
                }
                continue;
            }
            if !self.bolts.contains_key(&index) {
                let (Some(start), Some(initial)) = (ghost.position, p.initial.as_ref()) else {
                    continue;
                };
                // Its flight as its first update sends it whole: the bounce block (raw floats, in every first update: TGE's
                // `packUpdate` of all bits) over the initial block's 10-bit direction and centimeter start, which put the line
                // up to 0.18 degrees and a centimeter off the server's (decimeters at the far wall; a graze or a miss at a
                // corner).
                let (start, velocity) = match p.bounce {
                    Some((at, v)) => (at.map(|c| c.get()), v.map(|c| c.get())),
                    None => (
                        start,
                        initial.velocity.map_or([0.0; 3], |v| {
                            v.value(torque_ghosts::projectile::VELOCITY_NORMAL_BITS)
                        }),
                    ),
                };
                // Drawn from its source's muzzle as the frame draws it.
                let muzzle = initial.source.as_ref().and_then(|s| {
                    self.frames
                        .node_frame(world, &self.hooks, assets, s.object, s.slot as usize, -1)
                        .map(|(m, _)| m.column(3))
                });
                self.bolts.insert(
                    index,
                    Bolt {
                        flight: Flight {
                            start,
                            velocity,
                            age: 0.0,
                            muzzle,
                        },
                        trail: None,
                        ribbon: None,
                        stop: None,
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
                        r.add_point(self.style.projectile(&bolt.flight).column(3));
                        bolt.ribbon = Some(r);
                    }
                    None => bolt.trail = bolt_trail(world, db),
                }
            }
            let from = self.style.projectile(&bolt.flight).column(3);
            if !self.options.bolt_stops {
                bolt.flight.age += dt;
            } else if bolt.stop.is_none() {
                let line_from = bolt.flight.position();
                bolt.flight.age += dt;
                let line_to = bolt.flight.position();
                // Its line through the world as the client has it: stopped at the first surface it meets, where the server's
                // flight (the same line) explodes.
                let mid = [0, 1, 2].map(|k| 0.5 * (line_from[k] + line_to[k]));
                let reach = 0.5
                    * (0..3)
                        .map(|k| (line_to[k] - line_from[k]).powi(2))
                        .sum::<f32>()
                        .sqrt()
                    + 0.05;
                let mut faces = (input.surfaces)(world, mid, reach);
                faces.extend(self.interiors.faces_near(mid, reach));
                if let Some(t) = first_hit(line_from, line_to, &faces) {
                    let hit = [0, 1, 2].map(|k| line_from[k] + (line_to[k] - line_from[k]) * t);
                    bolt.flight.age -= dt * (1.0 - t);
                    bolt.stop = Some(hit);
                }
            }
            let (at, v) = (
                self.style.projectile(&bolt.flight).column(3),
                bolt.flight.velocity,
            );
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
                r.cut_past(end, bolt.flight.velocity);
                r.add_point(end);
            }
            r.dead = true;
            self.fading_ribbons.push(r);
        }
    }

    /// The frame's objects of the ghosts the engine draws ([`GhostOptions::select`]) as [`GhostScene::update`] left them, and
    /// the interiors' see-through panes; the map's interiors go into the view's static meshes on the way
    /// ([`crate::interiors`], each draw through [`GhostStyle::interior_draw`]).
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
        let mut out = view.ghost_objects(world, &shaped, set);
        let style = &mut self.style;
        out.extend(
            self.interiors
                .update(view, assets, world, &mut |m, d| style.interior_draw(m, d)),
        );
        out
    }

    /// Makes the interiors' meshes again at the next frame (the style's look of them changed).
    pub fn restyle_interiors(&mut self) {
        self.interiors.clear();
    }

    /// The frame's particles (the trails, the bursts, the images' emitters, the ribbons and the style's own, facing `eye`) and
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
        // The style's own at the muzzles the frame draws.
        let muzzles: Vec<Muzzle> = self
            .held
            .values()
            .filter_map(|h| {
                let (at, dir) = h.muzzle?;
                Some(Muzzle {
                    at,
                    dir,
                    since_fire: h.since_fire,
                })
            })
            .collect();
        let frame = EffectsFrame {
            eye: eye.position,
            right,
            up,
            muzzles: &muzzles,
        };
        draws.extend(self.style.effects(&frame, &mut set));
        draws.extend(self.dust.draws(eye.position, right, up, &mut set));
        let mut decals = self.decals.draws(eye.position, &mut set);
        if let Some(tracks) = &self.tracks {
            decals.extend(tracks.draws(eye.position, &mut set));
        }
        view.set_particles_and_decals(&draws, &decals, set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plain_style_draws_a_projectile_on_its_flight() {
        let mut f = Flight {
            start: [0.0; 3],
            velocity: [0.0, 100.0, 0.0],
            age: 0.0,
            muzzle: Some([0.2, 0.0, -0.1]),
        };
        let m = Plain.projectile(&f);
        assert_eq!(m.column(3), [0.0; 3]);
        assert!(
            (m.mul_v([0.0, 1.0, 0.0])[1] - 1.0).abs() < 1e-6,
            "along its flight"
        );
        f.age = 0.2;
        assert_eq!(Plain.projectile(&f).column(3), [0.0, 20.0, 0.0]);
        assert_eq!(f.flown(), 20.0);
    }

    /// Ghost 7 is deleted and another object made at index 7 between two frames (its birth 1 becomes 3); ghost 8 stays.
    #[test]
    fn a_ghost_reborn_on_its_index_starts_with_nothing_kept() {
        let mut scene = GhostScene::new(GhostOptions::default());
        scene.set_tire_tracks(Some(TrackRules {
            width: 0.4,
            spacing: 0.5,
            texture: "track.png".to_string(),
            lifetime: 10.0,
            fade: 4.0,
            max_segments: 20,
        }));
        let flat = |at: [f32; 3], _: f32| {
            let (x, y) = (at[0].floor(), at[1].floor());
            vec![
                [
                    [x - 2.0, y - 2.0, 0.0],
                    [x + 2.0, y - 2.0, 0.0],
                    [x + 2.0, y + 2.0, 0.0],
                ],
                [
                    [x - 2.0, y - 2.0, 0.0],
                    [x + 2.0, y + 2.0, 0.0],
                    [x - 2.0, y + 2.0, 0.0],
                ],
            ]
        };
        let wheel = |index: u32, y: f32| TrackWheel {
            key: (index, 0),
            contact: true,
            point: [0.0, y, 0.0],
        };
        let image = torque_ghost_world::images::MountedImage {
            datablock: 70,
            skin: torque_ghosts::NetStr::Null,
            wet: false,
            ammo: false,
            loaded: true,
            target: false,
            trigger: false,
            fire_count: 4,
            state: 2,
            delay: 0.0,
            at_ms: Some(0),
            ambient_time: 0.0,
            spin_scale: 0.0,
            spin_time: 0.0,
            anim: None,
            recoil: None,
            cap_hits: 0,
            events: Vec::new(),
        };
        let clock = torque_render_scene::images::advance_clock(&World::default(), &image, None);
        scene.forget_reborn(&BTreeMap::from([(7, 1), (8, 2)]));
        for g in [7, 8] {
            scene.hooks.clocks.lock().unwrap().insert((g, 0), clock);
            scene.held.insert(
                (g, 0),
                Held {
                    datablock: 70,
                    fire_count: 4,
                    since_fire: 0.5,
                    emitters: Vec::new(),
                    emit_ms: 0.0,
                    muzzle: None,
                },
            );
            scene.bolts.insert(
                g,
                Bolt {
                    flight: Flight {
                        start: [0.0; 3],
                        velocity: [0.0, 50.0, 0.0],
                        age: 0.3,
                        muzzle: None,
                    },
                    trail: None,
                    ribbon: None,
                    stop: None,
                },
            );
            scene.burst.insert(g);
        }
        let tracks = scene.tracks.as_mut().unwrap();
        for k in 0..4 {
            tracks.update(
                &[wheel(7, k as f32 * 0.5), wheel(8, k as f32 * 0.5)],
                &flat,
                0.016,
            );
        }
        let laid = tracks.len();

        scene.forget_reborn(&BTreeMap::from([(7, 3), (8, 2)]));
        let keys = |m: Vec<(u32, usize)>| m.into_iter().map(|k| k.0).collect::<Vec<_>>();
        let clocks = keys(scene.hooks.clocks.lock().unwrap().keys().copied().collect());
        assert_eq!(clocks, [8], "the new object's image clock starts anew");
        assert_eq!(
            keys(scene.held.keys().copied().collect()),
            [8],
            "and its held image"
        );
        assert_eq!(
            scene.bolts.keys().copied().collect::<Vec<_>>(),
            [8],
            "a projectile's flight"
        );
        assert_eq!(
            scene.burst.iter().copied().collect::<Vec<_>>(),
            [8],
            "its burst"
        );
        // The new object's wheel starts a new strip; ghost 8's runs on.
        let tracks = scene.tracks.as_mut().unwrap();
        tracks.update(&[wheel(7, 2.0), wheel(8, 2.0)], &flat, 0.016);
        assert_eq!(tracks.len(), laid + 1, "only ghost 8's strip runs on");
    }
}
