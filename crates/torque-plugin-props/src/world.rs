//! The server's props: one persistent Rapier world, the props in it by object id, their caps, sleep and freeze, the map pieces they
//! land on and the pushes of the players and vehicles that walk into them. Nothing here reads a script engine: the server's
//! glue ([`crate::server`]) feeds it and writes back what changed, so the world can be tested and benched alone.
//!
//! **A tick** ([`PropWorld::tick`]), after the process list has moved every object:
//! 1. each player's or vehicle's push on the props its box touches becomes an impulse ([`Pusher`]);
//! 2. the world takes one step of [`TICK`](torque_physics_rapier::TICK) seconds in its sub-steps (none when every prop sleeps or
//!    is frozen), charged to a budget: a step the budget refuses does not run, and the awake cap is lowered for the next ticks
//!    (the props farthest from every player go to sleep first);
//! 3. every awake prop's state is quantized to the wire's precision and written back ([`crate::quant`]); a prop slower than its
//!    datablock's thresholds for [`SLEEP_TICKS`] ticks falls asleep, stopped, and a sleeping prop another one wakes is awake
//!    again;
//! 4. past the awake cap, the props farthest from every player (then the highest ids) are put to sleep.
//!
//! What changed is reported ([`TickReport`]): the props whose state or flags must go to the clients, and the ones that fell
//! asleep or woke. Everything runs in id order over `BTreeMap`s, with no clock and no hash map, so a run repeats bit for bit on
//! every target.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_collide::math::{Box3, affine_inverse, mul_p};
use torque_collide::source::{next_revision, segment_box};
use torque_physics_rapier::{
    BodyDesc, BodyId, BodyState, Config, Error, Limits as WorldLimits, Motion, Shape, World,
};

use crate::data::{PropParams, PropShape};
use crate::hulls::PropHulls;
use crate::joints::{Joint, JointEvent, JointLimits};
use crate::quant::{self, Quantized};
use crate::records::{MASK_FLAGS, MASK_INIT, MASK_STATE};
pub use children::Link;

/// Ticks a prop must stay under its sleep thresholds before it falls asleep (half a second).
pub const SLEEP_TICKS: u16 = 16;
/// How far (in 1/1024 units) a sleeping prop may be nudged along an axis and still be put back to sleep.
const WAKE_STEPS: u32 = 2;
/// The most props a server holds (the slots of the collision source and the wire).
pub const MAX_PROPS: u32 = 1024;
/// The smallest half-extent of a pusher's kinematic box (units).
const MIN_HALF: f32 = 0.01;
/// The largest half-extent of a pusher's kinematic box (units): a box past it is no player or vehicle (a record is not trusted) and
/// is left out, counted in [`PropWorld::box_rejected`].
pub const MAX_PUSHER_HALF: f32 = 64.0;
/// The most map pieces and static shapes the world takes.
pub const MAX_STATICS: usize = 512;
/// How far past a pusher's box a prop is touched (units).
pub const PUSH_MARGIN: f32 = 0.08;
/// The share of a pusher's mass that drives a prop toward its speed in one tick (a light prop is carried along, a heavy one
/// barely moves).
pub const PUSH_SHARE: f32 = 0.5;
/// The fewest awake bodies a budget overrun lowers the cap to.
pub const MIN_AWAKE_CUT: u32 = 16;

mod children;

/// The caps a server holds its props to (its prefs, each held to its range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropLimits {
    /// Props one owner may have (1 to 1,024).
    pub per_player: u32,
    /// Props the owners of one remote address may have between them, orphans counted (1 to 1,024): a player who comes back
    /// under a new key starts no new quota.
    pub per_address: u32,
    /// Props on the server (1 to 1,024).
    pub per_server: u32,
    /// Props awake at once (1 to 1,024).
    pub awake: u32,
    /// Work units the step may take a tick (see [`torque_physics_rapier::work`]).
    pub step_budget: u64,
    /// Ticks an orphan (a prop whose owner left) lasts.
    pub orphan_ticks: u64,
}

impl Default for PropLimits {
    fn default() -> PropLimits {
        PropLimits {
            per_player: 64,
            per_address: 256,
            per_server: MAX_PROPS,
            awake: 256,
            step_budget: 4_000_000,
            orphan_ticks: 120 * 1000 / 32,
        }
    }
}

impl PropLimits {
    /// Every cap held to its range.
    pub fn held(self) -> PropLimits {
        PropLimits {
            per_player: self.per_player.clamp(1, MAX_PROPS),
            per_address: self.per_address.clamp(1, MAX_PROPS),
            per_server: self.per_server.clamp(1, MAX_PROPS),
            awake: self.awake.clamp(1, MAX_PROPS),
            step_budget: self.step_budget.clamp(1_000, 1_000_000_000),
            orphan_ticks: self.orphan_ticks.min(3600 * 1000 / 32),
        }
    }
}

/// The most step budgets the work of bodies made can owe (the debt past it is forgiven, [`PropWorld::forgiven_work`]); each step pays
/// half a budget of it.
pub const BUILD_DEBT_STEPS: u64 = 4;

/// The longest owner key text a game may give (`propOwnerKey`); a longer one is cut to it.
pub const MAX_OWNER_KEY: usize = 128;

/// What stands for a player who owns props: the game's own identity for them (its full text, at most [`MAX_OWNER_KEY`]
/// bytes, compared whole), the account id the auth provider verified for the connection, or the connection itself.
///
/// The three never equal one another, whatever bytes they hold: a game's text cannot name an account.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OwnerKey {
    /// The game's own text for the player (`propOwnerKey`).
    Named(Rc<[u8]>),
    /// The account id of the connection's verified identity: the same player on every connection.
    Identity(Rc<[u8]>),
    /// The connection (an anonymous player), by its object id.
    Connection(u32),
}

impl OwnerKey {
    /// The key of a game's text (cut to [`MAX_OWNER_KEY`] bytes), or of the connection when the text is empty.
    pub fn new(text: &[u8], conn: u32) -> OwnerKey {
        OwnerKey::resolve(text, None, conn)
    }

    /// The key of a connection: the game's text when it set one (it always wins), else the verified account id, else the
    /// connection.
    pub fn resolve(text: &[u8], identity: Option<&[u8]>, conn: u32) -> OwnerKey {
        if !text.is_empty() {
            OwnerKey::Named(Rc::from(&text[..text.len().min(MAX_OWNER_KEY)]))
        } else if let Some(id) = identity {
            OwnerKey::Identity(Rc::from(id))
        } else {
            OwnerKey::Connection(conn)
        }
    }
}

/// Who owns a prop: its key, the remote address it spawned from (the host, without the port number), and the connection that holds
/// it now, if it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub key: OwnerKey,
    pub address: Option<Rc<str>>,
    pub conn: Option<u32>,
    /// The tick the owner's connection went (none while it is there).
    pub left: Option<u64>,
}

/// One prop.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    /// The `PropData` datablock's object id, and its numbers.
    pub data: u32,
    pub params: PropParams,
    /// The hulls it collides as, when its datablock asked for the shape's and they were built ([`PropShape::Hulls`]).
    pub hulls: Option<Rc<PropHulls>>,
    /// The key of its collision shape on both sides (0 to 1,023).
    pub slot: u16,
    pub owner: Option<Owner>,
    /// The predict id the owner's client gave it (0: none).
    pub predict: u8,
    pub rest: Rest,
    /// Ticks it has stayed under its sleep thresholds.
    quiet: u16,
    /// Its state at the wire's precision, as the world holds it.
    pub q: Quantized,
    /// The pushers that touched it this tick.
    pub touched: Vec<u32>,
    /// How the scene hierarchy holds it ([`Link`]).
    pub link: Link,
}

/// Whether a prop moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rest {
    /// Simulated.
    #[default]
    Awake,
    /// Still, costing nothing until something wakes it.
    Asleep,
    /// Pinned by a script: a fixed body.
    Frozen,
}

impl Rest {
    pub fn is_awake(self) -> bool {
        self == Rest::Awake
    }

    pub fn is_asleep(self) -> bool {
        self == Rest::Asleep
    }

    pub fn is_frozen(self) -> bool {
        self == Rest::Frozen
    }
}

/// What [`PropWorld::spawn_with`] takes: the arguments of [`PropWorld::spawn`] and the hulls the prop collides as.
#[derive(Clone, Debug)]
pub struct SpawnArgs {
    pub id: u32,
    pub data: u32,
    pub params: PropParams,
    /// The hulls it collides as, when its `params` say [`PropShape::Hulls`]; otherwise they are not used.
    pub hulls: Option<Rc<PropHulls>>,
    /// A position and a unit quaternion `x, y, z, w`.
    pub pose: ([f32; 3], [f32; 4]),
    pub owner: Option<Owner>,
    pub predict: u8,
}

/// Why a spawn was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// The owner has the most props one player may have (of all kinds, or of this datablock).
    PerPlayer,
    /// The owners of the owner's remote address have the most props one address may have.
    PerAddress,
    /// The server has its most props.
    PerServer,
    /// A value was not finite, or the pose was out of the world.
    Bad,
    /// The object id is a prop already.
    Taken,
    /// The physics world refused it (its caps, or a panic that stopped it).
    World,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SpawnError::PerPlayer => "the player has the most props they may have",
            SpawnError::PerAddress => {
                "the players of this address have the most props they may have"
            }
            SpawnError::PerServer => "the server has the most props it may have",
            SpawnError::Bad => "the pose is not finite or is out of the world",
            SpawnError::Taken => "the object is a prop already",
            SpawnError::World => "the physics world refused the prop",
        })
    }
}

/// Something that pushes props: a player or a vehicle, as its box is after the tick, and the velocity it moves at (a player's: the
/// speed its move asks for, since it stops at the prop it walks into).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pusher {
    pub id: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub velocity: [f32; 3],
    pub mass: f32,
}

impl Pusher {
    fn center(&self) -> [f32; 3] {
        std::array::from_fn(|k| (self.min[k] + self.max[k]) * 0.5)
    }
}

/// Where a player at a joint's end is ([`PropWorld::set_players`]): its feet and the way it faces across the ground, from its
/// transform on the server and from its predicted box and transform in a client's prediction ([`PlayerAt::of_box`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerAt {
    /// The middle of its box's floor.
    pub position: [f32; 3],
    /// The way it faces across the ground, a unit vector.
    pub forward: [f32; 2],
}

impl PlayerAt {
    /// A player at `position` facing `forward` across the ground (made unit; straight along +y when it is not a direction).
    pub fn facing(position: [f32; 3], forward: [f32; 2]) -> PlayerAt {
        let len = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
        let forward = if len > 1.0e-6 && len.is_finite() {
            forward.map(|f| f / len)
        } else {
            [0.0, 1.0]
        };
        PlayerAt { position, forward }
    }

    /// A player whose box is `min..max` (its position the middle of the box's floor), facing `forward` ([`PlayerAt::facing`]):
    /// how a client's prediction reads its Player from a move.
    pub fn of_box(min: [f32; 3], max: [f32; 3], forward: [f32; 2]) -> PlayerAt {
        let floor = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, min[2]];
        PlayerAt::facing(floor, forward)
    }

    /// A player standing at `position`, facing +y.
    pub fn at(position: [f32; 3]) -> PlayerAt {
        PlayerAt {
            position,
            forward: [0.0, 1.0],
        }
    }

    /// The point `offset` of its own frame (right, forward, up from its feet) in the world.
    pub fn point(&self, offset: [f32; 3]) -> [f32; 3] {
        let [fx, fy] = self.forward;
        let [r, f, u] = offset;
        [
            self.position[0] + fy * r + fx * f,
            self.position[1] - fx * r + fy * f,
            self.position[2] + u,
        ]
    }

    /// Its turn about the up axis: the unit quaternion (`x, y, z, w`) that takes +y to where it faces, with no trigonometry
    /// (the same bits on every target).
    pub fn turn(&self) -> [f32; 4] {
        let [fx, fy] = self.forward;
        // Half way between +y and the facing: (0, 0, sin, 1 + cos) made unit, which is a half turn when it faces -y.
        let q = [0.0, 0.0, -fx, 1.0 + fy];
        let len = (q[2] * q[2] + q[3] * q[3]).sqrt();
        if len > 1.0e-6 {
            q.map(|c| c / len)
        } else {
            [0.0, 0.0, 1.0, 0.0]
        }
    }
}

/// A static piece of the map props land on: a terrain or an interior, or a box (a static shape).
#[derive(Clone, Debug)]
pub enum StaticPiece {
    Map(Rc<MapShape>),
    Box {
        position: [f32; 3],
        rotation: [f32; 4],
        half: [f32; 3],
    },
}

impl StaticPiece {
    /// A static object's box (its object box at its scale and transform), as both sides take static shapes; none for a
    /// transform whose turn is not a number.
    pub fn of_view(view: &torque_collide::ObjectView) -> Option<StaticPiece> {
        let m = view.obj_to_world;
        let (lo, hi) = (view.obj_box.min, view.obj_box.max);
        let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5 * view.scale[k]);
        let half: [f32; 3] = std::array::from_fn(|k| ((hi[k] - lo[k]) * 0.5 * view.scale[k]).abs());
        let position: [f32; 3] = std::array::from_fn(|r| {
            m[r * 4] * center[0]
                + m[r * 4 + 1] * center[1]
                + m[r * 4 + 2] * center[2]
                + m[r * 4 + 3]
        });
        let rows: torque_ts_vm::geometry::Mat4 =
            std::array::from_fn(|r| std::array::from_fn(|c| m[r * 4 + c]));
        let rotation = quant::conjugate(torque_ts_vm::geometry::quat_from_matrix(&rows));
        rotation
            .iter()
            .all(|v| v.is_finite())
            .then_some(StaticPiece::Box {
                position,
                rotation,
                half,
            })
    }
}

/// What happened to a prop in a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropEvent {
    Sleep,
    Wake,
}

/// What a tick did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickReport {
    /// The props whose ghost groups changed, in id order, with the mask bits to mark.
    pub dirty: Vec<(u32, u32)>,
    /// Props that fell asleep or woke, in id order.
    pub events: Vec<(u32, PropEvent)>,
    /// The step did not run: the budget refused it.
    pub over_budget: bool,
    /// The bodies made (and made again) since the last tick and their work, when that was past a quarter of the step budget: the
    /// next steps' budgets are charged it.
    pub built: Option<(u32, u64)>,
    /// Props put to sleep because more were awake than the cap allows.
    pub capped: u32,
    /// Props awake after the tick.
    pub awake: u32,
    /// The physics world was stopped by a caught panic and made again from the record ([`PropWorld::rebuild_if_stopped`]).
    pub rebuilt: bool,
    /// Props the world made again did not take, taken out (their objects are for the caller to delete).
    pub lost: Vec<u32>,
    /// Props taken out because the world would not put them back after a change (their objects are for the caller to delete).
    pub dropped: Vec<u32>,
    /// The children whose state changed, in id order: written to their records without marking them, as a child is drawn from
    /// its parent.
    pub moved: Vec<u32>,
    /// The joints whose ghost groups changed, in id order, with the mask bits to mark.
    pub joints: Vec<(u32, u32)>,
    /// Joints that broke, in id order.
    pub joint_events: Vec<(u32, JointEvent)>,
    /// The work the step was charged (its budget's units).
    pub work: u64,
}

/// The server's props ([module docs](self)).
pub struct PropWorld {
    world: World,
    props: BTreeMap<u32, Prop>,
    slots: BTreeSet<u16>,
    /// The prop in each slot in use.
    by_slot: BTreeMap<u16, u32>,
    statics: BTreeMap<u32, StaticPiece>,
    /// The joints by object id ([`crate::joints`]).
    pub(crate) joints: BTreeMap<u32, Joint>,
    /// The children by kind ([`children::Kids`]).
    kids: children::Kids,
    /// Roots whose bodies changed in a way their children must follow, for the hierarchy's next fit.
    dirty_roots: Vec<u32>,
    /// Work spent this tick on bodies made for the hierarchy, against the step's budget.
    build_spent: u64,
    /// Props the world could not put back after a change that failed, taken out for the tick to report.
    dropped: Vec<u32>,
    /// Props whose ghost groups change for a reason of the hierarchy (made a child, freed), for the next tick to report.
    changed: Vec<(u32, u32)>,
    /// The joints taken out because a body at an end went, with their ropes, for the caller to delete their objects.
    pub(crate) gone_joints: Vec<(u32, u32)>,
    /// Where each player at a grapple's or a hold's end is, given before each tick.
    pub(crate) players: BTreeMap<u32, PlayerAt>,
    /// Props whose flags (held) changed since the last tick, for the next to report.
    pub(crate) reflag: Vec<u32>,
    /// Holds broken since the last tick (their props were frozen), for the next to report.
    pub(crate) broken_holds: Vec<u32>,
    /// The players' and vehicles' boxes as the last tick had them (each with its id): what a held prop is kept out of.
    pub(crate) bodies: Vec<(u32, [f32; 3], [f32; 3])>,
    /// Each pusher's kinematic box in the physics world, by its id: its middle and half-extents as last placed
    /// ([`PropWorld::place_pushers`]).
    pusher_boxes: BTreeMap<u32, ([f32; 3], [f32; 3])>,
    /// The boxes that block without pushing ([`PropWorld::set_bystanders`]), kept until set again.
    bystanders: Vec<Pusher>,
    /// Sleepers a joint woke since the last tick (reported by the next).
    roused: Vec<u32>,
    pub joint_limits: JointLimits,
    config: Config,
    /// The awake cap a budget overrun lowered, while it lasts.
    cut: Option<u32>,
    pub limits: PropLimits,
    /// The work of the bodies made (and made again) since the last step: it is charged to the next step's budget.
    built_work: u64,
    /// How many bodies that work was of.
    built_bodies: u32,
    /// Ticks the budget refused.
    pub over_budget: u64,
    /// Work of bodies made that was never charged to a step, because a burst left more than [`BUILD_DEBT_STEPS`] steps' budgets
    /// owing (the excess is forgiven, here counted).
    pub forgiven_work: u64,
    /// Pusher and bystander boxes the physics world refused to make (its caps; each is tried again at the next tick).
    pub box_refused: u64,
    /// Boxes left out for not being a body's: not finite, or a half-extent past [`MAX_PUSHER_HALF`].
    pub box_rejected: u64,
    /// Boxes the physics world could not take because it had stopped (it is made again at the next tick).
    pub box_stopped: u64,
    /// Physics worlds made again after a caught panic.
    pub rebuilds: u64,
    /// Physics calls that failed where nothing was there to tell, for the tick to print ([`PropWorld::note`]).
    problems: Vec<String>,
    /// A revision of the props' poses ([`torque_collide::source::next_revision`]): a new one at every spawn, removal and pose
    /// change, so the collision source lists the props again only after one.
    revision: u64,
}

impl std::fmt::Debug for PropWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropWorld")
            .field("props", &self.props.len())
            .field("statics", &self.statics.len())
            .field("joints", &self.joints.len())
            .finish_non_exhaustive()
    }
}

/// The physics world's settings for props: `substeps` (1 to 8) a tick, room for every prop, the map's statics and pushers, and a
/// map shape as wide as a mission's floor.
pub fn config(substeps: u8) -> Config {
    Config {
        substeps: substeps.clamp(1, torque_physics_rapier::limits::MAX_SUBSTEPS),
        limits: WorldLimits {
            max_bodies: MAX_PROPS as usize + MAX_STATICS,
            max_half_extent: 4096.0,
            max_speed: quant::MAX_SPEED,
            max_angular_speed: quant::MAX_SPIN,
            max_coordinate: quant::MAX_COORD,
            ..WorldLimits::default()
        },
        ..Config::default()
    }
}

/// A pusher's box as a kinematic body takes it: its middle and half-extents. `None` when it is not a body's: not finite, or a
/// half-extent past [`MAX_PUSHER_HALF`].
fn pusher_box(p: &Pusher) -> Option<([f32; 3], [f32; 3])> {
    let middle: [f32; 3] = std::array::from_fn(|k| (p.min[k] + p.max[k]) * 0.5);
    let half: [f32; 3] = std::array::from_fn(|k| ((p.max[k] - p.min[k]) * 0.5).max(MIN_HALF));
    let sane = middle.iter().chain(&half).all(|v| v.is_finite())
        && half.iter().all(|h| *h <= MAX_PUSHER_HALF);
    sane.then_some((middle, half))
}

/// Adds a static piece to `world`; false when the world refused it.
fn add_static(world: &mut World, id: u32, piece: &StaticPiece) -> bool {
    match piece {
        StaticPiece::Map(shape) => world.add_map(ObjectId(id), Rc::clone(shape)).is_ok(),
        &StaticPiece::Box {
            position,
            rotation,
            half,
        } => {
            let desc = BodyDesc {
                motion: Motion::Fixed,
                shape: Shape::Box { half },
                position,
                rotation,
                friction: torque_physics_rapier::world::MAP_FRICTION,
                ..BodyDesc::default()
            };
            let mut free = |_: u64| true;
            world.add_body(BodyId(id), &desc, &mut free).is_ok()
        }
    }
}

/// The body of a prop's shape: its hulls when it has them, else the box or ball its datablock gave.
fn body_shape(params: &PropParams, hulls: Option<&PropHulls>) -> Shape {
    match (params.shape, hulls) {
        (PropShape::Hulls { .. }, Some(hulls)) => hulls.shape(),
        (PropShape::Box(half) | PropShape::Hulls { half, .. }, _) => Shape::Box { half },
        (PropShape::Ball(radius), _) => Shape::Ball { radius },
    }
}

/// The half-extents along the world's axes of a box `half` turned by the unit quaternion `q`.
pub fn world_half(half: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let m = quant::matrix([0.0; 3], q);
    std::array::from_fn(|r| (0..3).map(|c| (m[r * 4 + c] * half[c]).abs()).sum::<f32>())
}

/// A vector's length: the one form every length in this crate is taken by (the sum in `x, y, z` order, then the root), so a
/// length is the same bits wherever it is read.
pub(crate) fn length(v: [f32; 3]) -> f32 {
    torque_collide::math::dot(v, v).sqrt()
}

/// The most problems the world keeps for the tick to print ([`PropWorld::take_problems`]).
const MAX_PROBLEMS: usize = 64;

impl PropWorld {
    /// An empty world of `substeps` sub-steps a tick.
    pub fn new(substeps: u8, limits: PropLimits) -> PropWorld {
        PropWorld {
            world: World::new(config(substeps)),
            props: BTreeMap::new(),
            slots: (0..MAX_PROPS as u16).collect(),
            by_slot: BTreeMap::new(),
            statics: BTreeMap::new(),
            joints: BTreeMap::new(),
            kids: children::Kids::default(),
            dirty_roots: Vec::new(),
            build_spent: 0,
            dropped: Vec::new(),
            changed: Vec::new(),
            gone_joints: Vec::new(),
            players: BTreeMap::new(),
            reflag: Vec::new(),
            broken_holds: Vec::new(),
            bodies: Vec::new(),
            pusher_boxes: BTreeMap::new(),
            bystanders: Vec::new(),
            roused: Vec::new(),
            joint_limits: JointLimits::default(),
            config: config(substeps),
            cut: None,
            limits: limits.held(),
            built_work: 0,
            built_bodies: 0,
            over_budget: 0,
            forgiven_work: 0,
            box_refused: 0,
            box_rejected: 0,
            box_stopped: 0,
            rebuilds: 0,
            problems: Vec::new(),
            revision: next_revision(),
        }
    }

    /// Notes a physics call that failed, for the tick to print: not when the thing was gone already (a grapple or a broken
    /// joint has no physics joint) or the world has stopped (it is made again from the props' states, and that is printed).
    pub(crate) fn note<T>(&mut self, what: &str, result: Result<T, Error>) {
        match result {
            Ok(_) | Err(Error::Unknown(_) | Error::UnknownJoint(_) | Error::Poisoned) => {}
            Err(e) => {
                if self.problems.len() < MAX_PROBLEMS {
                    self.problems.push(format!("props: {what}: {e}"));
                }
            }
        }
    }

    /// The physics calls that failed since the last call where no caller was there to tell (not a stopped world, which its
    /// rebuild reports).
    pub fn take_problems(&mut self) -> Vec<String> {
        std::mem::take(&mut self.problems)
    }

    /// The physics world.
    pub fn physics(&self) -> &World {
        &self.world
    }

    /// The revision of the props' slots and poses: it changes whenever a prop is spawned, removed or moves.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The props by id.
    pub fn props(&self) -> &BTreeMap<u32, Prop> {
        &self.props
    }

    pub fn get(&self, id: u32) -> Option<&Prop> {
        self.props.get(&id)
    }

    /// The prop whose collision shape is keyed `slot`.
    pub fn in_slot(&self, slot: u16) -> Option<u32> {
        self.by_slot.get(&slot).copied()
    }

    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Props awake now.
    pub fn awake(&self) -> u32 {
        self.props
            .values()
            .filter(|p| p.rest.is_awake() && p.link == Link::Free)
            .count() as u32
    }

    /// The props `key` owns.
    pub fn owned_by(&self, key: &OwnerKey) -> u32 {
        self.props
            .values()
            .filter(|p| p.owner.as_ref().is_some_and(|o| &o.key == key))
            .count() as u32
    }

    /// The props the owners of `address` have, orphans counted.
    pub fn owned_from(&self, address: &str) -> u32 {
        self.props
            .values()
            .filter(|p| {
                p.owner
                    .as_ref()
                    .and_then(|o| o.address.as_deref())
                    .is_some_and(|a| a == address)
            })
            .count() as u32
    }

    /// Whether `owner` may have one more prop of datablock `data` (with `params`): its own cap, its address's and the
    /// datablock's per-player cap.
    pub fn may_own(&self, owner: &Owner, data: u32, params: &PropParams) -> Result<(), SpawnError> {
        if self.owned_by(&owner.key) >= self.limits.per_player {
            return Err(SpawnError::PerPlayer);
        }
        if let Some(a) = owner.address.as_deref()
            && self.owned_from(a) >= self.limits.per_address
        {
            return Err(SpawnError::PerAddress);
        }
        let kind = self
            .props
            .values()
            .filter(|p| p.data == data && p.owner.as_ref().is_some_and(|po| po.key == owner.key))
            .count() as u32;
        if params.max_per_player > 0 && kind >= params.max_per_player {
            return Err(SpawnError::PerPlayer);
        }
        Ok(())
    }

    /// Adds a prop as object `id` of datablock `data` at a pose (a position and a unit quaternion `x, y, z, w`), owned by `owner`. Refused past a cap (the owner's, the datablock's
    /// per-player one, the server's) or for a pose that is not finite or is out of the world.
    pub fn spawn(
        &mut self,
        id: u32,
        data: u32,
        params: PropParams,
        pose: ([f32; 3], [f32; 4]),
        owner: Option<Owner>,
        predict: u8,
    ) -> Result<(), SpawnError> {
        self.spawn_with(SpawnArgs {
            id,
            data,
            params,
            hulls: None,
            pose,
            owner,
            predict,
        })
    }

    /// [`PropWorld::spawn`] of a prop that may collide as hulls ([`SpawnArgs::hulls`]).
    pub fn spawn_with(&mut self, args: SpawnArgs) -> Result<(), SpawnError> {
        let SpawnArgs {
            id,
            data,
            params,
            hulls,
            pose: (position, rotation),
            owner,
            predict,
        } = args;
        if self.props.contains_key(&id)
            || self.statics.contains_key(&id)
            || self.joints.contains_key(&id)
        {
            return Err(SpawnError::Taken);
        }
        let finite = position.iter().chain(&rotation).all(|v| v.is_finite());
        if !finite || position.iter().any(|v| v.abs() > quant::MAX_COORD) {
            return Err(SpawnError::Bad);
        }
        if self.props.len() as u32 >= self.limits.per_server {
            return Err(SpawnError::PerServer);
        }
        if let Some(o) = &owner {
            self.may_own(o, data, &params)?;
        }
        let Some(&slot) = self.slots.iter().next() else {
            return Err(SpawnError::PerServer);
        };
        let start = Quantized::of(&BodyState {
            position,
            rotation,
            ..BodyState::default()
        });
        let s = start.state();
        let desc = BodyDesc {
            motion: Motion::Dynamic,
            shape: body_shape(&params, hulls.as_deref()),
            position: s.position,
            rotation: s.rotation,
            density: params.density,
            friction: params.friction,
            restitution: params.restitution,
            linear_damping: params.linear_damping,
            angular_damping: params.angular_damping,
            ..BodyDesc::default()
        };
        let mut work = 0u64;
        let mut count = |units: u64| {
            work = work.saturating_add(units);
            true
        };
        let added = self.world.add_body(BodyId(id), &desc, &mut count);
        self.built_work = self.built_work.saturating_add(work);
        self.built_bodies = self.built_bodies.saturating_add(1);
        match added {
            Ok(()) => {}
            Err(Error::NonFinite) => return Err(SpawnError::Bad),
            Err(_) => return Err(SpawnError::World),
        }
        self.slots.remove(&slot);
        self.by_slot.insert(slot, id);
        self.revision = next_revision();
        self.props.insert(
            id,
            Prop {
                data,
                params,
                hulls,
                slot,
                owner,
                predict,
                rest: Rest::Awake,
                quiet: 0,
                q: start,
                touched: Vec::new(),
                link: Link::Free,
            },
        );
        Ok(())
    }

    /// The work of bodies made that the next steps' budgets are still to be charged.
    pub fn owed_work(&self) -> u64 {
        self.built_work
    }

    /// Takes a prop away; true when it was one.
    pub fn remove(&mut self, id: u32) -> bool {
        if !self.props.contains_key(&id) {
            return false;
        }
        // What is a part of this prop's body leaves it as a body of its own, where it is, at the speed its point had.
        self.free_parts(id, false);
        self.dirty_roots.push(id);
        let Some(p) = self.props.remove(&id) else {
            return false;
        };
        self.unindex(id, p.link);
        self.revision = next_revision();
        self.slots.insert(p.slot);
        self.by_slot.remove(&p.slot);
        let removed = match p.link {
            Link::Part { .. } => self.world.remove_part(BodyId(id)),
            _ => self.world.remove_body(BodyId(id)),
        };
        self.note(&format!("prop {id}'s body"), removed);
        self.drop_joints_of(id);
        true
    }

    /// The body again, as `motion`, at the prop's state (frozen: a fixed body; thawed: a dynamic one at rest).
    fn rebuild(&mut self, id: u32, motion: Motion) -> bool {
        if self.props.get(&id).is_none_or(|p| p.link.is_child()) {
            return false;
        }
        // Frozen, a prop is no body a part can sit on: what hung on it is driven where it is.
        self.free_parts(id, motion == Motion::Fixed);
        self.dirty_roots.push(id);
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        let s = p.q.stopped().state();
        let desc = BodyDesc {
            motion,
            shape: body_shape(&p.params, p.hulls.as_deref()),
            position: s.position,
            rotation: s.rotation,
            density: p.params.density,
            friction: p.params.friction,
            restitution: p.params.restitution,
            linear_damping: p.params.linear_damping,
            angular_damping: p.params.angular_damping,
            ..BodyDesc::default()
        };
        let before = BodyDesc {
            motion: if motion == Motion::Fixed {
                Motion::Dynamic
            } else {
                Motion::Fixed
            },
            linvel: p.q.state().linvel,
            angvel: p.q.state().angvel,
            ..desc.clone()
        };
        let removed = self.world.remove_body(BodyId(id));
        self.note(&format!("prop {id}'s body"), removed);
        let mut work = 0u64;
        let mut free = |units: u64| {
            work = work.saturating_add(units);
            true
        };
        let (made, has_body) = match self.world.add_body(BodyId(id), &desc, &mut free) {
            Ok(()) => (true, true),
            Err(e) => {
                // The old body goes back as it was. When the world refuses that too it has stopped, and the next tick
                // makes every body (and joint) again from its prop's state.
                self.note(&format!("prop {id}'s new body"), Err::<(), _>(e));
                let back = self.world.add_body(BodyId(id), &before, &mut free);
                let has_body = back.is_ok();
                self.note(&format!("prop {id}'s body put back"), back);
                (false, has_body)
            }
        };
        self.built_work = self.built_work.saturating_add(work);
        self.built_bodies = self.built_bodies.saturating_add(1);
        // Removing the body took its joints out of the physics world: they go back on the body it has now.
        if has_body {
            self.reinsert_joints_of(id);
        }
        made
    }

    /// Pins a prop where it is (a fixed body); false when it is not a prop, is frozen already, or its datablock says it cannot be.
    /// The mask bits to send are returned with `true`.
    pub fn freeze(&mut self, id: u32) -> Option<u32> {
        let p = self.props.get(&id)?;
        if p.rest.is_frozen() || !p.params.can_freeze || p.link.is_child() {
            return None;
        }
        if !self.rebuild(id, Motion::Fixed) {
            return None;
        }
        let p = self.props.get_mut(&id)?;
        p.rest = Rest::Frozen;
        p.q = p.q.stopped();
        self.revision = next_revision();
        // A frozen prop cannot be held: its hold breaks.
        self.break_holds_on(id);
        Some(MASK_STATE | MASK_FLAGS)
    }

    /// Lets a frozen prop move again (awake, at rest); the mask bits to send.
    pub fn unfreeze(&mut self, id: u32) -> Option<u32> {
        let p = self.props.get(&id)?;
        if !p.rest.is_frozen() {
            return None;
        }
        if !self.rebuild(id, Motion::Dynamic) {
            return None;
        }
        let p = self.props.get_mut(&id)?;
        p.rest = Rest::Awake;
        p.quiet = 0;
        Some(MASK_STATE | MASK_FLAGS)
    }

    /// Puts an awake prop to sleep where it is, stopped, as a sleeper of its own falls asleep (a map's props placed at rest:
    /// nothing shifts as the map loads); the mask bits to send. None for a frozen prop, one asleep already, or one that is not
    /// there. It reports no [`PropEvent::Sleep`] (the prop did not come to rest by itself), stops a prop in mid-air as well,
    /// and ignores `can_freeze` (a sleeper is not pinned). A push, an impulse, a joint or a prop that moves into it wakes it
    /// as it wakes any sleeper.
    pub fn sleep(&mut self, id: u32) -> Option<u32> {
        let p = self.props.get(&id)?;
        if !p.rest.is_awake() || p.link.is_child() {
            return None;
        }
        self.put_to_sleep(id);
        Some(MASK_STATE | MASK_FLAGS)
    }

    /// Adds an impulse to a prop (held to the world's limits); wakes it. False for a frozen prop or one that is not there.
    pub fn impulse(&mut self, id: u32, impulse: [f32; 3]) -> bool {
        let Some(body) = self.pushable(id) else {
            return false;
        };
        self.world.apply_impulse(BodyId(body), impulse).is_ok()
    }

    /// The body an impulse on prop `id` is given to: its own, or its root's for a part of it. None for a prop that is not there,
    /// is frozen, or is driven.
    fn pushable(&self, id: u32) -> Option<u32> {
        let p = self.props.get(&id)?;
        match p.link {
            Link::Free if !p.rest.is_frozen() => Some(id),
            Link::Part { root, .. } => Some(root),
            Link::Free | Link::Driven { .. } => None,
        }
    }

    /// Puts a prop back to the state `q` (a client restoring a server's state), awake or asleep as `rest` says, its quiet count
    /// started over; false for a frozen prop, one that is not there, or a `rest` of [`Rest::Frozen`].
    pub fn restore(&mut self, id: u32, q: Quantized, rest: Rest) -> bool {
        let Some(p) = self
            .props
            .get_mut(&id)
            .filter(|p| !p.rest.is_frozen() && !p.link.is_child())
        else {
            return false;
        };
        if rest.is_frozen() {
            return false;
        }
        p.q = q;
        self.revision = next_revision();
        p.quiet = 0;
        p.rest = rest;
        let mut s = q.state();
        s.sleeping = rest.is_asleep();
        self.world.set_body(BodyId(id), &s).is_ok()
    }

    /// [`PropWorld::impulse`] at the world point `point` (off its center it turns the prop too).
    pub fn impulse_at(&mut self, id: u32, impulse: [f32; 3], point: [f32; 3]) -> bool {
        let Some(body) = self.pushable(id) else {
            return false;
        };
        self.world
            .apply_impulse_at(BodyId(body), impulse, point)
            .is_ok()
    }

    /// Sets a prop's speed and spin (held to its datablock's top speed and the world's spin); wakes it.
    pub fn set_velocity(&mut self, id: u32, linvel: [f32; 3], angvel: [f32; 3]) -> bool {
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        if p.rest.is_frozen()
            || p.link.is_child()
            || !linvel.iter().chain(&angvel).all(|v| v.is_finite())
        {
            return false;
        }
        let max = p.params.max_speed;
        let speed = length(linvel);
        let linvel = if speed > max {
            linvel.map(|v| v * (max / speed))
        } else {
            linvel
        };
        let Some(now) = self.world.body(BodyId(id)) else {
            return false;
        };
        let s = BodyState {
            linvel,
            angvel,
            sleeping: false,
            ..now
        };
        self.world.set_body(BodyId(id), &s).is_ok()
    }

    /// Changes who owns a prop (the init group goes again, so each client hears whether it is its own).
    pub fn set_owner(&mut self, id: u32, owner: Option<Owner>) -> Option<u32> {
        let p = self.props.get_mut(&id)?;
        p.owner = owner;
        Some(MASK_INIT)
    }

    /// The map's statics now, by object id: the [`MAX_STATICS`] lowest ids are kept (pieces past them are left out), pieces
    /// that are new among them are added and ones that went are taken away.
    pub fn sync_statics(&mut self, mut pieces: Vec<(u32, StaticPiece)>) {
        pieces.sort_by_key(|(id, _)| *id);
        pieces.dedup_by_key(|(id, _)| *id);
        pieces.retain(|(id, _)| !self.props.contains_key(id));
        pieces.truncate(MAX_STATICS);
        let wanted: BTreeSet<u32> = pieces.iter().map(|(id, _)| *id).collect();
        let gone: Vec<u32> = self
            .statics
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect();
        for id in gone {
            match self.statics.remove(&id) {
                Some(StaticPiece::Map(_)) => {
                    let _ = self.world.remove_map(ObjectId(id));
                }
                Some(StaticPiece::Box { .. }) => {
                    let _ = self.world.remove_body(BodyId(id));
                }
                None => {}
            }
        }
        for (id, piece) in pieces {
            if self.statics.contains_key(&id) {
                continue;
            }
            if add_static(&mut self.world, id, &piece) {
                self.statics.insert(id, piece);
            }
        }
    }

    /// The ids of the statics in the world, in order.
    pub fn static_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.statics.keys().copied()
    }

    /// The physics world, for a caller that needs more than this type gives (a test that stops it, a tool). Bodies changed
    /// through it are not the props' own record: the next tick writes the record back over them.
    pub fn physics_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// The settings a physics world made again ([`PropWorld::rebuild_if_stopped`]) is made with: the world's own at first.
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// A physics world made again from the record, after a caught panic stopped the old one: the statics, then every prop in
    /// id order at its state at the wire's precision (asleep ones asleep, frozen ones fixed). A prop the new world does not
    /// take (its caps) is taken out; the ids of those are returned, for their objects to be deleted. None when the world was
    /// not stopped.
    pub fn rebuild_if_stopped(&mut self) -> Option<Vec<u32>> {
        if !self.world.is_poisoned() {
            return None;
        }
        self.world = World::new(self.config);
        self.pusher_boxes.clear();
        let mut lost = Vec::new();
        let statics = std::mem::take(&mut self.statics);
        for (id, piece) in statics {
            if add_static(&mut self.world, id, &piece) {
                self.statics.insert(id, piece);
            }
        }
        let ids: Vec<u32> = self.props.keys().copied().collect();
        for id in ids {
            let Some(p) = self.props.get(&id) else {
                continue;
            };
            // A part is put on its root once every body is there.
            let motion = match p.link {
                Link::Part { .. } => continue,
                Link::Driven { .. } => Motion::Kinematic,
                Link::Free if p.rest.is_frozen() => Motion::Fixed,
                Link::Free => Motion::Dynamic,
            };
            let s = p.q.state();
            let desc = BodyDesc {
                motion,
                shape: body_shape(&p.params, p.hulls.as_deref()),
                position: s.position,
                rotation: s.rotation,
                linvel: s.linvel,
                angvel: s.angvel,
                density: p.params.density,
                friction: p.params.friction,
                restitution: p.params.restitution,
                linear_damping: p.params.linear_damping,
                angular_damping: p.params.angular_damping,
                ..BodyDesc::default()
            };
            let asleep = p.rest.is_asleep();
            let mut work = 0u64;
            let mut count = |units: u64| {
                work = work.saturating_add(units);
                true
            };
            let added = self.world.add_body(BodyId(id), &desc, &mut count);
            self.built_work = self.built_work.saturating_add(work);
            self.built_bodies = self.built_bodies.saturating_add(1);
            if added.is_err() {
                self.remove(id);
                lost.push(id);
                continue;
            }
            if asleep {
                let mut st = s;
                st.sleeping = true;
                let _ = self.world.set_body(BodyId(id), &st);
            }
        }
        let parts: Vec<(u32, u32, children::Pose)> = self
            .props
            .iter()
            .filter_map(|(&id, p)| match p.link {
                Link::Part { root, local } => Some((id, root, local)),
                _ => None,
            })
            .collect();
        for (id, root, local) in parts {
            if !self.put_part(id, root, local, false) {
                self.remove(id);
                lost.push(id);
            }
        }
        self.reinsert_all_joints();
        self.rebuilds += 1;
        Some(lost)
    }

    /// The statics in the world.
    pub fn statics(&self) -> usize {
        self.statics.len()
    }

    /// The pushes of `pushers` on the props their boxes touch, as impulses toward each pusher's speed.
    fn push(&mut self, pushers: &[Pusher]) {
        for p in self.props.values_mut() {
            p.touched.clear();
        }
        let mut impulses: Vec<(u32, [f32; 3])> = Vec::new();
        let held: BTreeSet<u32> = self.held_props().collect();
        // What a push on a part moves is its root: that body's mass, speed and top speed.
        let roots: BTreeMap<u32, (f32, [f32; 3], f32)> = self
            .props
            .values()
            .filter_map(|p| match p.link {
                Link::Part { root, .. } => Some(root),
                _ => None,
            })
            .filter_map(|root| {
                let r = self.props.get(&root)?;
                let mass = self.world.mass_of(BodyId(root))?;
                Some((root, (mass, r.q.state().linvel, r.params.max_speed)))
            })
            .collect();
        for (&id, prop) in self.props.iter_mut() {
            // A held prop goes where its hold drives it.
            if held.contains(&id) {
                continue;
            }
            let center = prop.q.position();
            let half = world_half(prop.params.shape.half_extents(), prop.q.rotation());
            for pusher in pushers {
                let touching = (0..3).all(|k| {
                    center[k] + half[k] + PUSH_MARGIN >= pusher.min[k]
                        && center[k] - half[k] - PUSH_MARGIN <= pusher.max[k]
                });
                if !touching {
                    continue;
                }
                prop.touched.push(pusher.id);
                if prop.rest.is_frozen() || matches!(prop.link, Link::Driven { .. }) {
                    continue;
                }
                let c = pusher.center();
                let to = [center[0] - c[0], center[1] - c[1], 0.0];
                let reach = length(to);
                let v = [pusher.velocity[0], pusher.velocity[1], 0.0];
                let speed = length(v);
                if !(reach > 1.0e-4 && speed > 0.05 && speed.is_finite()) {
                    continue;
                }
                let dir = v.map(|x| x / speed);
                // Only a push toward the prop: a pusher walking away or past it leaves it.
                if dir[0] * to[0] / reach + dir[1] * to[1] / reach < 0.2 {
                    continue;
                }
                // A part is pushed as the body it is on.
                let (target, mass, now, max_speed) = match prop.link {
                    Link::Part { root, .. } => match roots.get(&root) {
                        Some(&(mass, now, max)) => (root, mass, now, max),
                        None => continue,
                    },
                    _ => (
                        id,
                        prop.params.mass(),
                        prop.q.state().linvel,
                        prop.params.max_speed,
                    ),
                };
                let speed = speed.min(max_speed);
                let short = speed - (now[0] * dir[0] + now[1] * dir[1]);
                if short <= 0.0 {
                    continue;
                }
                let share = (pusher.mass.max(0.0) * PUSH_SHARE / mass.max(1.0e-3)).min(1.0);
                let j = mass * short * share;
                impulses.push((target, dir.map(|d| d * j)));
            }
        }
        for (id, j) in impulses {
            let _ = self.world.apply_impulse(BodyId(id), j);
        }
    }

    /// One server tick ([module docs](self)): `pushers` push, the world steps, the state is quantized and written back, the
    /// sleepers and the cap applied. `near` gives a prop's distance to the nearest player (for the cap's order).
    pub fn tick(&mut self, pushers: &[Pusher], near: &dyn Fn([f32; 3]) -> f32) -> TickReport {
        let rebuilt = self.rebuild_if_stopped();
        let mut report = TickReport {
            rebuilt: rebuilt.is_some(),
            lost: rebuilt.unwrap_or_default(),
            ..TickReport::default()
        };
        let boxes: Cow<'_, [Pusher]> = if self.bystanders.is_empty() {
            Cow::Borrowed(pushers)
        } else {
            let mut all: Vec<Pusher> = pushers.iter().chain(&self.bystanders).copied().collect();
            all.sort_by_key(|p| p.id);
            all.dedup_by_key(|p| p.id);
            Cow::Owned(all)
        };
        self.bodies = boxes.iter().map(|p| (p.id, p.min, p.max)).collect();
        self.place_pushers(&boxes);
        self.push(pushers);
        self.joints_before_step(&mut report);
        report.dirty.append(&mut self.changed);
        for id in std::mem::take(&mut self.reflag) {
            if self.props.contains_key(&id) {
                report.dirty.push((id, MASK_FLAGS));
            }
        }
        for id in std::mem::take(&mut self.roused) {
            report.events.push((id, PropEvent::Wake));
            report.dirty.push((id, MASK_FLAGS));
        }
        let budget = self.limits.step_budget;
        // What the bodies made since the last step cost counts against the next steps' budgets, at most half of one a step, so a
        // script that makes a great many at once slows the steps that follow a little rather than stopping one.
        let bodies = std::mem::take(&mut self.built_bodies);
        if self.built_work > budget / 4 {
            report.built = Some((bodies, self.built_work));
        }
        // A burst cannot hold the steps after it at half a budget for long: the debt is at most BUILD_DEBT_STEPS budgets (paid
        // off in twice that many steps), and the rest is forgiven.
        let kept = self.built_work.min(budget.saturating_mul(BUILD_DEBT_STEPS));
        self.forgiven_work = self.forgiven_work.saturating_add(self.built_work - kept);
        self.built_work = kept;
        let owed = self.built_work.min(budget / 2);
        self.built_work -= owed;
        let mut spent = owed;
        let mut charge = |units: u64| {
            spent = spent.saturating_add(units);
            spent <= budget
        };
        let cap = self.limits.awake;
        // With every prop asleep or frozen (and nothing woken by a push or a call) the world has nothing to move: the step is
        // left out, so a server of sleepers costs only the walk below.
        let moving = self.props.iter().any(|(&id, p)| {
            !p.rest.is_frozen() && self.world.body(BodyId(id)).is_some_and(|s| !s.sleeping)
        });
        let stepped = if moving {
            self.world.step(&mut charge).map(|_| ())
        } else {
            Ok(())
        };
        report.work = spent;
        match stepped {
            Ok(_) => {
                // A cut eases back up an eighth of the cap a tick.
                self.cut = self
                    .cut
                    .map(|c| c.saturating_add((cap / 8).max(1)))
                    .filter(|&c| c < cap);
            }
            Err(Error::Exhausted) => {
                report.over_budget = true;
                self.over_budget += 1;
                let awake = self.awake();
                self.cut = Some((awake.min(cap) * 3 / 4).max(MIN_AWAKE_CUT));
            }
            Err(_) => {}
        }
        let ids: Vec<u32> = self.props.keys().copied().collect();
        let held: BTreeSet<u32> = self.held_props().collect();
        for id in ids {
            self.settle(id, held.contains(&id), &mut report);
        }
        self.settle_children(&mut report);
        // The cap: the farthest awake props from every player sleep first.
        let cap = self.cut.unwrap_or(cap).min(cap);
        let mut awake: Vec<(f32, u32)> = self
            .props
            .iter()
            .filter(|(_, p)| p.rest.is_awake() && p.link == Link::Free)
            .map(|(&id, p)| (near(p.q.position()), id))
            .collect();
        if awake.len() as u32 > cap {
            // Farthest first; a distance that is not a number counts as the farthest; ties by the higher id.
            awake.sort_by(|a, b| {
                let key = |d: f32| if d.is_nan() { f32::INFINITY } else { d };
                key(b.0).total_cmp(&key(a.0)).then(b.1.cmp(&a.1))
            });
            let over = awake.len() - cap as usize;
            let mut sleepers: Vec<u32> = awake.iter().take(over).map(|&(_, id)| id).collect();
            sleepers.sort_unstable();
            for id in sleepers {
                self.put_to_sleep(id);
                report.capped += 1;
                report.events.push((id, PropEvent::Sleep));
                report.dirty.push((id, MASK_STATE | MASK_FLAGS));
            }
        }
        report.dirty.sort_by_key(|&(id, _)| id);
        report.dirty.dedup_by(|b, a| {
            if a.0 == b.0 {
                a.1 |= b.1;
                true
            } else {
                false
            }
        });
        report.events.sort_by_key(|&(id, _)| id);
        self.joints_after_step(&mut report);
        report.awake = self.awake();
        report.dropped = std::mem::take(&mut self.dropped);
        self.build_spent = 0;
        report
    }

    /// The first prop the segment from `from` to `to` meets (its box at its state now, at the wire's precision), but `ignore`,
    /// and the share of the way along the segment it meets it. Held props count.
    pub fn ray(&self, from: [f32; 3], to: [f32; 3], ignore: Option<u32>) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for (&id, p) in &self.props {
            if Some(id) == ignore {
                continue;
            }
            let to_local = affine_inverse(&quant::matrix(p.q.position(), p.q.rotation()));
            let half = p.params.shape.half_extents();
            let bx = Box3::new(half.map(|h| -h), half);
            if let Some((t, ..)) = segment_box(mul_p(&to_local, from), mul_p(&to_local, to), &bx)
                && best.is_none_or(|(_, b)| t < b)
            {
                best = Some((id, t));
            }
        }
        best
    }

    /// Boxes that block props and keep a held prop out like a pusher's, but push nothing: the other players and vehicles a
    /// client's prediction knows only by their records. Kept, in place of the last, until set again; each is a pusher's box in
    /// every other way ([`PropWorld::place_pushers`]). Boxes are made in ascending id order (the physics world's rule), a
    /// second with an id taken is dropped.
    pub fn set_bystanders(&mut self, mut boxes: Vec<Pusher>) {
        boxes.sort_by_key(|p| p.id);
        boxes.dedup_by_key(|p| p.id);
        self.bystanders = boxes;
    }

    /// Each pusher's box as a kinematic body of the physics world (frictionless, so a player walking on a prop does not drag
    /// it), placed where its box is now and moved only when the box moves: no prop can be pushed into a player or a vehicle
    /// (a held prop's shove, another prop's fall), and one that is there already is pushed out by the solver, walls and other
    /// props in its way. A box goes when its pusher does. Pushes stay [`Pusher`]'s impulses: a box is placed, never given a
    /// speed, so it flings nothing (a teleported figure included).
    fn place_pushers(&mut self, pushers: &[Pusher]) {
        // A pusher whose box is no longer a body's (not finite, or huge) is gone as well: its old body does not stay where it was.
        let gone: Vec<u32> = self
            .pusher_boxes
            .keys()
            .filter(|id| {
                !pushers
                    .iter()
                    .any(|p| p.id == **id && pusher_box(p).is_some())
            })
            .copied()
            .collect();
        for id in gone {
            self.pusher_boxes.remove(&id);
            let removed = self.world.remove_body(BodyId(id));
            self.note(&format!("pusher {id}'s box"), removed);
        }
        for p in pushers {
            // A pusher whose id is a prop, a map piece or a joint is no box (ids are the host's objects: never so).
            if self.props.contains_key(&p.id)
                || self.statics.contains_key(&p.id)
                || self.joints.contains_key(&p.id)
            {
                continue;
            }
            let Some((middle, half)) = pusher_box(p) else {
                self.box_rejected += 1;
                continue;
            };
            match self.pusher_boxes.get(&p.id) {
                Some(&(m, h)) if m == middle && h == half => {}
                Some(&(_, h)) if h == half => {
                    let s = BodyState {
                        position: middle,
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        linvel: [0.0; 3],
                        angvel: [0.0; 3],
                        sleeping: false,
                    };
                    let set = self.world.set_body(BodyId(p.id), &s);
                    self.note(&format!("pusher {}'s box", p.id), set);
                    self.pusher_boxes.insert(p.id, (middle, half));
                }
                kept => {
                    // New, or its box changed size (a crouch): made again.
                    if kept.is_some() {
                        let removed = self.world.remove_body(BodyId(p.id));
                        self.note(&format!("pusher {}'s box", p.id), removed);
                    }
                    let desc = BodyDesc {
                        motion: Motion::Kinematic,
                        shape: Shape::Box { half },
                        position: middle,
                        friction: 0.0,
                        restitution: 0.0,
                        ..BodyDesc::default()
                    };
                    let mut free = |_: u64| true;
                    let made = self.world.add_body(BodyId(p.id), &desc, &mut free);
                    if made.is_ok() {
                        self.pusher_boxes.insert(p.id, (middle, half));
                    } else {
                        self.pusher_boxes.remove(&p.id);
                        match made {
                            Err(Error::Poisoned) => self.box_stopped += 1,
                            _ => self.box_refused += 1,
                        }
                    }
                    self.note(&format!("pusher {}'s box", p.id), made);
                }
            }
        }
    }

    /// A hold on prop `id` was made or taken away: its flags go to the clients at the next tick, and the collision sources list
    /// the props again (a held prop is left out of the players' collision).
    pub(crate) fn held_changed(&mut self, id: u32) {
        self.revision = next_revision();
        self.reflag.push(id);
    }

    /// Wakes a sleeping prop that something other than a contact is to move (a joint made or changed, a winch reeling): awake,
    /// its quiet count started over, its wake reported by the next tick. A prop that sleeps settles back exactly where it lay
    /// unless it moves, so a joint's slow start would otherwise never wake it.
    pub(crate) fn rouse(&mut self, id: u32) {
        let Some(p) = self.props.get_mut(&id) else {
            return;
        };
        if !p.rest.is_asleep() {
            return;
        }
        p.rest = Rest::Awake;
        p.quiet = 0;
        let woke = self.world.wake(BodyId(id));
        self.note(&format!("waking prop {id}"), woke);
        self.roused.push(id);
    }

    /// A prop stopped where it is and asleep.
    fn put_to_sleep(&mut self, id: u32) {
        let Some(p) = self.props.get_mut(&id) else {
            return;
        };
        p.q = p.q.stopped();
        self.revision = next_revision();
        p.rest = Rest::Asleep;
        p.quiet = 0;
        let mut s = p.q.state();
        s.sleeping = true;
        let _ = self.world.set_body(BodyId(id), &s);
    }

    /// One prop after the step: quantized and written back, asleep or woken (a `held` one never falls asleep).
    fn settle(&mut self, id: u32, held: bool, report: &mut TickReport) {
        let Some(s) = self.world.body(BodyId(id)) else {
            return;
        };
        let Some(p) = self.props.get_mut(&id) else {
            return;
        };
        if p.rest.is_frozen() || p.link.is_child() {
            return;
        }
        if p.rest.is_asleep() {
            if s.sleeping {
                return;
            }
            // The world wakes a sleeper now and then on its own (a contact it re-checks): unless it really moves, it is put back
            // exactly as it lay.
            let still = Quantized::of(&s);
            let lin = length(s.linvel);
            let ang = length(s.angvel);
            let moved = still
                .pos
                .iter()
                .zip(&p.q.pos)
                .any(|(a, b)| a.abs_diff(*b) > WAKE_STEPS);
            if !moved
                && lin < p.params.sleep_linear.max(0.05)
                && ang < p.params.sleep_angular.max(0.05)
            {
                let mut back = p.q.state();
                back.sleeping = true;
                let _ = self.world.set_body(BodyId(id), &back);
                return;
            }
            p.rest = Rest::Awake;
            p.quiet = 0;
            report.events.push((id, PropEvent::Wake));
            report.dirty.push((id, MASK_FLAGS));
        }
        if s.sleeping {
            // The world put it to sleep on its own: stopped where it lies.
            self.put_to_sleep(id);
            report.events.push((id, PropEvent::Sleep));
            report.dirty.push((id, MASK_STATE | MASK_FLAGS));
            return;
        }
        let q = Quantized::of(&s);
        let (lin, ang) = (length(s.linvel), length(s.angvel));
        let quiet = !held
            && p.params.sleep_linear > 0.0
            && p.params.sleep_angular > 0.0
            && lin < p.params.sleep_linear
            && ang < p.params.sleep_angular;
        p.quiet = if quiet { p.quiet.saturating_add(1) } else { 0 };
        if p.quiet >= SLEEP_TICKS {
            p.q = q;
            self.revision = next_revision();
            self.put_to_sleep(id);
            report.events.push((id, PropEvent::Sleep));
            report.dirty.push((id, MASK_STATE | MASK_FLAGS));
            return;
        }
        let changed = q != p.q;
        p.q = q;
        if changed {
            self.revision = next_revision();
        }
        let _ = self.world.set_body(BodyId(id), &q.state());
        if changed {
            report.dirty.push((id, MASK_STATE));
        }
    }

    /// The props' states as words, in id order (the id, the thirteen integers, the flags), for a trace hash.
    pub fn state_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.props.len() * 15);
        for (&id, p) in &self.props {
            out.push(id);
            out.extend(p.q.words().map(|w| w as u32));
            let link = match p.link {
                Link::Free => 0,
                Link::Driven { .. } => 1,
                Link::Part { .. } => 2,
            };
            out.push(
                u32::from(p.rest.is_asleep()) | u32::from(p.rest.is_frozen()) << 1 | link << 2,
            );
        }
        out
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
