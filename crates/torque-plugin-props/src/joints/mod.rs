//! Joints between props, the world and players: what a server holds two things together with, in the props' physics world.
//!
//! **Kinds** ([`Kind`]):
//! - a **tether** keeps its ends at most its length apart (and at least its shortest, when it has one): hard (a limit), or soft
//!   (a spring of `stiffness` and `damping` past the length, [`Spring`]);
//! - a **winch** is a tether whose length moves to a target at a rate, pulling with at most a force ([`Winch`]); its length is
//!   state, sent to every client;
//! - a **weld** holds two props (or a prop and the world) at the pose they had to each other when it was made;
//! - a **hinge** lets two props (or a prop and the world) turn about one axis through one point, within angle limits when it
//!   has some, driven by a motor when it has one ([`Hinge`]);
//! - a **grapple** is a tether from a player (a Player, simulated by Torque's rules, not a physics body) to a point or a prop;
//!   a player has at most one.
//!   It pulls inside the Player's own tick ([`torque_game::player_tether`]), so a client's prediction replays it exactly; a prop
//!   at the far end takes the equal and opposite impulse the next step.
//! - a **hold** carries a prop at a point of a player's own frame (its offset: right, forward and up from its feet, turned
//!   with its heading), upright and facing where the player faces: before each step the prop is driven to reach that pose by
//!   the step's end ([`HOLD_SPEED`] and [`HOLD_SPIN`] at most), so walls and other props still stop it, and never into
//!   another player's box. Nothing pulls the player, and only the holder's own collision passes through its held prop
//!   ([`torque_collide::source::CollisionShape::passes`]), so a held prop never pushes its holder and every other player meets
//!   it. A player holds at most one prop and a prop has at most one holder; a frozen prop cannot be held; a held prop never
//!   sleeps. A hold breaks when its prop is frozen or ends a step more than [`HOLD_BREAK`] from its point (stuck behind a
//!   wall). Every player's and vehicle's box is a kinematic body of the props' world, so no prop (held, let go or shoved by
//!   one) ends up inside a player.
//!
//! Every joint but a grapple and a hold is a Rapier impulse joint in the props' world ([`torque_physics_rapier::JointKind`]),
//! added when it is made and again, in id order, when the world is made again. A joint whose body goes goes with it; one pulled
//! past its `breakForce` breaks (it holds nothing from then on, and `onBreak` is called on it), and stays until a script
//! deletes it.
//!
//! **Caps** ([`JointLimits`]): joints per player (64), per address (256), per server (1,024) and per prop (16); a length is at
//! most [`MAX_LENGTH`], a winch's rate at most [`MAX_WINCH_RATE`], every force, torque and spring held to its range, and a point
//! or an offset past [`MAX_COORD`](crate::quant::MAX_COORD) refused.
//!
//! The server's console functions are [`natives`]; the `Joint` ghost class and what a client keeps of it, [`records`] and
//! [`client`]. Joints have no client entry point: a game's own server commands decide who may join what, checking the ends with
//! `jointCheck` first.

pub mod client;
pub mod natives;
pub mod records;
pub mod server;
mod world;

#[cfg(feature = "rope")]
pub mod rope;

use crate::world::Owner;

pub use torque_physics_rapier::{Motor, Spring};
pub use world::point_on;

/// The name of the ghost class a joint is.
pub const GHOST_CLASS: &str = "Joint";

/// The longest a tether, a winch or a grapple may be (units).
pub const MAX_LENGTH: f32 = 200.0;
/// The shortest a tether's length may be (units).
pub const MIN_LENGTH: f32 = torque_physics_rapier::world::joint::MIN_DISTANCE;
/// The fastest a winch reels (units a second).
pub const MAX_WINCH_RATE: f32 = 20.0;
/// The largest break force, winch force and motor torque (force units).
pub const MAX_FORCE: f32 = 1.0e7;
/// The stiffest spring and the most damping a tether takes (a grapple's are the Player tether's own caps).
pub const MAX_STIFFNESS: f32 = torque_game::player_tether::MAX_STIFFNESS;
pub const MAX_DAMPING: f32 = torque_game::player_tether::MAX_DAMPING;
/// The fastest a hinge's motor turns (radians a second).
pub const MAX_MOTOR_SPEED: f32 = 50.0;
/// The fastest a hold drives its prop to its point (units a second): a holder that turns fast swings its prop no faster, so it
/// never tunnels through a wall or flings what it hits.
pub const HOLD_SPEED: f32 = 10.0;
/// The fastest a hold turns its prop to face where its holder faces (radians a second).
pub const HOLD_SPIN: f32 = 20.0;
/// How far a held prop may end a step from its point before its hold breaks (units).
pub const HOLD_BREAK: f32 = 1.0;
/// The most joints a server holds (the cap of its pref).
pub const MAX_JOINTS: u32 = 1024;
/// How near its length a tether's ends must be to count as at its limit (units).
pub const AT_LIMIT: f32 = 0.01;

/// The caps a server holds its joints to (its prefs, each held to its range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JointLimits {
    /// Joints one owner may have (1 to 1,024).
    pub per_player: u32,
    /// Joints the owners of one remote address may have between them, orphans counted (1 to 1,024): a player who comes back
    /// under a new key starts no new quota.
    pub per_address: u32,
    /// Joints on the server (1 to 1,024).
    pub per_server: u32,
    /// Joints one prop may be an end of (1 to 64).
    pub per_prop: u32,
}

impl Default for JointLimits {
    fn default() -> JointLimits {
        JointLimits {
            per_player: 64,
            per_address: 256,
            per_server: MAX_JOINTS,
            per_prop: 16,
        }
    }
}

impl JointLimits {
    /// Every cap held to its range.
    #[must_use]
    pub fn held(self) -> JointLimits {
        JointLimits {
            per_player: self.per_player.clamp(1, MAX_JOINTS),
            per_address: self.per_address.clamp(1, MAX_JOINTS),
            per_server: self.per_server.clamp(1, MAX_JOINTS),
            per_prop: self.per_prop.clamp(1, 64),
        }
    }
}

/// One end of a joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    /// A prop, at `local` in its own frame.
    Prop { id: u32, local: [f32; 3] },
    /// A point in the world.
    Point([f32; 3]),
    /// A player (a grapple's or a hold's first end): a grapple's point is its position moved by `offset`, a hold's is `offset`
    /// in the player's own frame (right, forward, up from its feet).
    Player { id: u32, offset: [f32; 3] },
    /// The world itself: a weld's second end, held to the world as it was when the weld was made. It goes on the wire as the
    /// world's origin.
    World,
}

impl End {
    /// The prop at this end, if it is one.
    pub fn prop(&self) -> Option<u32> {
        match *self {
            End::Prop { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// How far a tether's ends may be apart: at most `max` (its length now), at least `min`, and with a spring past `max` when it
/// is soft.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub min: f32,
    pub max: f32,
    pub spring: Option<Spring>,
}

impl Reach {
    /// A hard reach of `max` (held to [`MAX_LENGTH`]).
    pub fn new(max: f32) -> Reach {
        Reach {
            min: 0.0,
            max,
            spring: None,
        }
        .held()
    }

    /// The reach with every value in its range (a value that is not a number: the range's low end).
    #[must_use]
    pub fn held(self) -> Reach {
        let max = number(self.max, MIN_LENGTH, MAX_LENGTH);
        Reach {
            min: number(self.min, 0.0, max),
            max,
            spring: self.spring.map(held_spring).filter(|s| s.stiffness > 0.0),
        }
    }
}

/// What is at a grapple's far end, for its pull ([`grapple_pull`]): something that does not move (a point, a frozen prop, a
/// prop that is not there), or a prop of a mass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FarEnd {
    Fixed,
    Prop(f32),
}

impl FarEnd {
    /// A prop: fixed while frozen.
    pub fn of(prop: &crate::world::Prop) -> FarEnd {
        if prop.rest.is_frozen() {
            FarEnd::Fixed
        } else {
            FarEnd::Prop(prop.params.mass())
        }
    }
}

/// The impulse a grapple's pull gives the prop at its far end when the tether changed its Player's velocity by `change` (the
/// Player `player_mass` heavy): the momentum of the pair's reduced mass, so the two end the pull moving apart no faster than
/// they were (a light prop is not flung past its Player). Toward a fixed end it is the Player's own momentum, which is also
/// what a pull counts toward a break force ([`grapple_momentum`]). None for a value that is not finite.
pub fn grapple_pull(change: [f32; 3], player_mass: f32, far: FarEnd) -> Option<[f32; 3]> {
    let m = number(player_mass, 0.0, 1.0e6);
    let mass = match far {
        FarEnd::Prop(p) => {
            let p = number(p, 0.0, 1.0e6);
            if m + p > 0.0 { m * p / (m + p) } else { m }
        }
        FarEnd::Fixed => m,
    };
    let impulse = change.map(|c| -c * mass);
    impulse.iter().all(|v| v.is_finite()).then_some(impulse)
}

/// The Player's momentum a grapple's pull took (its velocity change `change` times its `player_mass`): what the pull counts
/// toward the grapple's break force, whatever is at the far end. None for a value that is not finite.
pub fn grapple_momentum(change: [f32; 3], player_mass: f32) -> Option<[f32; 3]> {
    grapple_pull(change, player_mass, FarEnd::Fixed)
}

/// A number held to `lo..=hi`, a NaN to `lo`.
pub(crate) fn number(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() { lo } else { v.clamp(lo, hi) }
}

/// A spring with its values in their ranges.
pub(crate) fn held_spring(s: Spring) -> Spring {
    Spring {
        stiffness: number(s.stiffness, 0.0, MAX_STIFFNESS),
        damping: number(s.damping, 0.0, MAX_DAMPING),
    }
}

/// A winch: the length moves to `target` at `rate` units a second, and stops shortening while the joint pulls with more than
/// `max_force` (0: no limit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Winch {
    pub target: f32,
    pub rate: f32,
    pub max_force: f32,
}

impl Winch {
    #[must_use]
    pub fn held(self) -> Winch {
        Winch {
            target: number(self.target, MIN_LENGTH, MAX_LENGTH),
            rate: number(self.rate, 0.0, MAX_WINCH_RATE),
            max_force: number(self.max_force, 0.0, MAX_FORCE),
        }
    }
}

/// A hinge: the axis in each prop's frame (the world's for a world end), its angle limits (radians, low then high) and its
/// motor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hinge {
    pub axis_a: [f32; 3],
    pub axis_b: [f32; 3],
    pub limits: Option<[f32; 2]>,
    pub motor: Option<Motor>,
}

/// What a joint holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Tether {
        reach: Reach,
        winch: Option<Winch>,
    },
    /// The second end's pose in the first's frame when the weld was made.
    Weld {
        frame: ([f32; 3], [f32; 4]),
    },
    Hinge(Hinge),
    Grapple {
        reach: Reach,
    },
    /// A player carries the prop at its second end ([module docs](self)).
    Hold,
}

/// The kinds as the wire numbers them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum KindId {
    #[default]
    Tether,
    Winch,
    Weld,
    Hinge,
    Grapple,
    Hold,
}

impl KindId {
    /// Every kind, by its number.
    pub const ALL: [KindId; 6] = [
        KindId::Tether,
        KindId::Winch,
        KindId::Weld,
        KindId::Hinge,
        KindId::Grapple,
        KindId::Hold,
    ];

    pub fn number(self) -> u32 {
        self as u32
    }

    /// The kind of a name (`jointGetKind`'s, whatever its case).
    pub fn named(name: &str) -> Option<KindId> {
        KindId::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(name))
    }

    /// The kind of a number; a number that names none is a tether.
    pub fn of(n: u32) -> KindId {
        KindId::ALL
            .get(n as usize)
            .copied()
            .unwrap_or(KindId::Tether)
    }

    /// The name scripts see (`jointKind`).
    pub fn name(self) -> &'static str {
        match self {
            KindId::Tether => "tether",
            KindId::Winch => "winch",
            KindId::Weld => "weld",
            KindId::Hinge => "hinge",
            KindId::Grapple => "grapple",
            KindId::Hold => "hold",
        }
    }

    /// Whether the kind's first end is a player.
    pub fn from_player(self) -> bool {
        matches!(self, KindId::Grapple | KindId::Hold)
    }

    /// Whether the kind has a length a rope following it takes.
    pub fn has_reach(self) -> bool {
        matches!(self, KindId::Tether | KindId::Winch | KindId::Grapple)
    }
}

impl Kind {
    pub fn id(&self) -> KindId {
        match self {
            Kind::Tether { winch: None, .. } => KindId::Tether,
            Kind::Tether { winch: Some(_), .. } => KindId::Winch,
            Kind::Weld { .. } => KindId::Weld,
            Kind::Hinge(_) => KindId::Hinge,
            Kind::Grapple { .. } => KindId::Grapple,
            Kind::Hold => KindId::Hold,
        }
    }

    /// The reach of a tether, a winch or a grapple.
    pub fn reach(&self) -> Option<&Reach> {
        match self {
            Kind::Tether { reach, .. } | Kind::Grapple { reach } => Some(reach),
            _ => None,
        }
    }

    pub(crate) fn reach_mut(&mut self) -> Option<&mut Reach> {
        match self {
            Kind::Tether { reach, .. } | Kind::Grapple { reach } => Some(reach),
            _ => None,
        }
    }
}

/// What a joint did in the last tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The distance between its ends.
    pub distance: f32,
    /// The force and the torque it held its ends with.
    pub force: f32,
    pub torque: f32,
    /// A tether's ends are at its length (a rope following it is drawn taut).
    pub at_limit: bool,
    /// It was pulled past its break force: it holds nothing.
    pub broken: bool,
}

/// One joint.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub kind: Kind,
    pub a: End,
    pub b: End,
    /// The force (or torque, at one unit) past which it breaks; 0 never.
    pub break_force: f32,
    /// Who owns it: their key, their remote address and connection, as a prop's owner ([`Owner`]); an owner who leaves
    /// leaves it an orphan for `$Pref::Props::OrphanSeconds`, as their props.
    pub owner: Option<Owner>,
    pub status: Status,
    /// The networked rope that shows it (0: none).
    pub rope: u32,
    /// A grapple's pull on the prop at its far end, given at the next step, and the force it pulled its player with this
    /// tick.
    pub(crate) pending: [f32; 3],
    pub(crate) pull: f32,
}

impl Joint {
    /// A joint of `kind` between `a` and `b`, owned by `owner`, that never breaks.
    pub fn new(kind: Kind, a: End, b: End, owner: Option<Owner>) -> Joint {
        Joint {
            kind,
            a,
            b,
            break_force: 0.0,
            owner,
            status: Status::default(),
            rope: 0,
            pending: [0.0; 3],
            pull: 0.0,
        }
    }

    /// The props at its ends.
    pub fn props(&self) -> impl Iterator<Item = u32> + '_ {
        [self.a.prop(), self.b.prop()].into_iter().flatten()
    }
}

/// Why a joint was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointError {
    /// The owner has the most joints one player may have.
    PerPlayer,
    /// The owners of the owner's remote address have the most joints one address may have.
    PerAddress,
    /// The server has its most joints.
    PerServer,
    /// A prop at an end is an end of the most joints one prop may be.
    PerProp,
    /// The player has a grapple already (one each, so the server and the client predicting it always pick the same).
    Grappled,
    /// The player holds a prop already, or the prop has a holder already (one each).
    Held,
    /// A frozen prop cannot be held.
    Frozen,
    /// An end is not one the kind takes (a prop, a point, a grapple's player first, a weld's or a hinge's world second), a point
    /// or an offset is out of the world, or both ends are one thing or both points.
    Ends,
    /// A value was not finite.
    Bad,
    /// The object id is a joint already.
    Taken,
    /// The physics world refused it.
    World,
}

impl std::fmt::Display for JointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            JointError::PerPlayer => "the player has the most joints they may have",
            JointError::PerAddress => {
                "the players of this address have the most joints they may have"
            }
            JointError::PerServer => "the server has the most joints it may have",
            JointError::PerProp => "a prop at an end has the most joints it may have",
            JointError::Grappled => "the player has a grapple already",
            JointError::Held => "the player holds a prop already, or the prop is held",
            JointError::Frozen => "a frozen prop cannot be held",
            JointError::Ends => {
                "the ends must be two things: props, points, or a player and a prop or point"
            }
            JointError::Bad => "a value is not finite",
            JointError::Taken => "the object is a joint already",
            JointError::World => "the physics world refused the joint",
        })
    }
}

impl std::error::Error for JointError {}

/// What happened to a joint in a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointEvent {
    /// It was pulled past its break force.
    Break,
}
