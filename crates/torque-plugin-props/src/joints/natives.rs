//! The server's console functions for joints (`jointTether` and kin) and the check a game calls before it joins two things for a
//! player (`jointCheck`). Joints have no client entry point: a game's own server commands decide who may join what.
//!
//! An *end* is written as text: a prop (its name or id: its middle), a prop and a point in its own frame (`"%prop x y z"`), or a
//! point in the world (`"x y z"`). A grapple's first end is a player, with an offset from its position (`"%player 0 0 1.2"`) when
//! the point held is not its feet. A weld's or a hinge's second end may be left empty: the world.

use torque_engine::args::vec3;
use torque_engine::{Res, ScriptError, VmMut, function};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::server::{CLASS, joint_of, make, write_back};
use super::{End, Hinge, Joint, JointError, Kind, KindId, MAX_LENGTH, Motor, Reach, Spring, Winch};
use crate::server::{ServerProps, is_class, owner_key, owner_of as server_owner_of};
use crate::world::{Owner, PropWorld, length};

/// `"x y z"` as finite numbers.
fn finite(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let v = vec3(text, what)?;
    if v.iter().all(|c| c.is_finite()) {
        Ok(v)
    } else {
        Err(ScriptError::new(format!("{what} must be finite")))
    }
}

fn number(v: f32, what: &str) -> Result<f32, ScriptError> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ScriptError::new(format!("{what} must be a finite number")))
    }
}

/// Reads an end's text ([module docs](self)): empty is the world.
fn parse_end(vm: &Vm, world: &PropWorld, text: &str, what: &str) -> Result<End, ScriptError> {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    let object = |name: &str| -> Result<u32, ScriptError> {
        vm.objects()
            .find(name.as_bytes())
            .ok_or_else(|| ScriptError::new(format!("{what}: there is no object {name:?}")))
    };
    let thing = |id: u32, at: [f32; 3]| {
        if world.get(id).is_some() {
            Ok(End::Prop { id, local: at })
        } else if torque_game::sim::player::state(vm, id).is_some() {
            Ok(End::Player { id, offset: at })
        } else {
            Err(ScriptError::new(format!(
                "{what}: {id} is not a prop or a player"
            )))
        }
    };
    match words.as_slice() {
        [] => Ok(End::World),
        [name] => thing(object(name)?, [0.0; 3]),
        [name, x, y, z] => thing(object(name)?, finite(&format!("{x} {y} {z}"), what)?),
        [_, _, _] => Ok(End::Point(finite(text, what)?)),
        _ => Err(ScriptError::new(format!(
            "{what} must be a prop, \"prop x y z\" or \"x y z\""
        ))),
    }
}

/// The owner of a joint a connection given by id asks for (none for 0 or an object that is not there): its key and address, as a
/// prop's ([`crate::server::owner_of`]).
fn owner_of(vm: &mut Vm, owner: Option<i32>) -> Option<Owner> {
    owner
        .and_then(|o| ObjId::try_from(o).ok())
        .filter(|&o| o != 0 && vm.objects().contains(o))
        .map(|o| server_owner_of(vm, o))
}

/// Makes a joint, the error a script sees when it was refused.
fn made(vm: &mut Vm, props: &ServerProps, joint: Joint) -> Result<i32, ScriptError> {
    make(vm, props, joint)
        .map(|id| id as i32)
        .map_err(ScriptError::new)
}

/// ties two ends together, at most `length` apart, and returns the joint's object id
///
/// Each end is a prop (`"%prop"`, its middle; `"%prop x y z"`, a point in its own frame) or a point in the world (`"x y z"`); at
/// least one is a prop. The tether is hard (the ends are never more than `length` apart) until `jointSetSpring` makes it a spring
/// past its length; `jointSetLength` gives it a shortest too, `jointWinch` makes it reel. `length` is held to 0.001 to 200. `owner`
/// is the connection (a client) the joint counts against (`$Pref::Props::MaxJointsPerPlayer`). Past a cap the call prints an
/// error and returns 0.
#[function(name = "jointTether")]
fn joint_tether(
    mut vm: VmMut,
    props: Res<ServerProps>,
    a: &str,
    b: &str,
    length: f32,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let length = number(length, "the length")?;
    let (a, b) = ends(&mut vm, &props, a, b)?;
    let owner = owner_of(&mut vm, owner);
    let kind = Kind::Tether {
        reach: Reach::new(length),
        winch: None,
    };
    made(&mut vm, &props, Joint::new(kind, a, b, owner))
}

/// The two ends read against the server's world.
fn ends(vm: &mut Vm, props: &ServerProps, a: &str, b: &str) -> Result<(End, End), ScriptError> {
    let mut srv = props.0.borrow_mut();
    let world = srv.world_mut(vm);
    Ok((
        parse_end(vm, world, a, "the first end")?,
        parse_end(vm, world, b, "the second end")?,
    ))
}

/// tethers a player to a point or a prop, at most `length` away, and returns the joint's object id
///
/// `player` is a Player, held at its position (`"%player x y z"`: moved by an offset, a hand's height say); `target` is a point
/// (`"x y z"`) or a prop (`"%prop"`, `"%prop x y z"`). The pull runs inside the Player's own tick, so its client predicts it
/// exactly; a prop at the far end is pulled back the other way. `jointSetSpring` makes it a spring past its length,
/// `jointSetLength` reels it.
#[function(name = "jointGrapple")]
fn joint_grapple(
    mut vm: VmMut,
    props: Res<ServerProps>,
    player: &str,
    target: &str,
    length: f32,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let length = number(length, "the length")?;
    let (a, b) = ends(&mut vm, &props, player, target)?;
    if !matches!(a, End::Player { .. }) || matches!(b, End::World | End::Player { .. }) {
        return Err(ScriptError::new(
            "a grapple is from a player to a point or a prop",
        ));
    }
    let owner = owner_of(&mut vm, owner);
    let kind = Kind::Grapple {
        reach: Reach::new(length),
    };
    made(&mut vm, &props, Joint::new(kind, a, b, owner))
}

/// makes a player carry a prop and returns the joint's object id
///
/// `offset` is `"x y z"` in the player's own frame (right, forward and up from its feet): the prop is carried there, upright
/// and facing where the player faces, driven each step to that point so walls and other props still stop it, and never
/// pushing its holder (the players' collision leaves a held prop out). A player holds one prop and a prop has one holder;
/// `jointDelete` lets go. What may be held, from how far and by whom is the game's own rule (`jointCheck` with kind `hold`
/// first): there is no client entry point.
#[function(name = "jointHold")]
fn joint_hold(
    mut vm: VmMut,
    props: Res<ServerProps>,
    player: &str,
    prop: &str,
    offset: &str,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let offset = finite(offset, "the offset")?;
    let (a, b) = ends(&mut vm, &props, player, prop)?;
    let (End::Player { id, .. }, End::Prop { id: p, .. }) = (a, b) else {
        return Err(ScriptError::new("a hold is a player's on a prop"));
    };
    let owner = owner_of(&mut vm, owner);
    let joint = Joint::new(
        Kind::Hold,
        End::Player { id, offset },
        End::Prop {
            id: p,
            local: [0.0; 3],
        },
        owner,
    );
    made(&mut vm, &props, joint)
}

/// welds a prop to another (or, with `b` empty, to the world) as they are now, and returns the joint's object id
#[function(name = "jointWeld")]
fn joint_weld(
    mut vm: VmMut,
    props: Res<ServerProps>,
    a: &str,
    b: &str,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let (na, nb) = ends(&mut vm, &props, a, b)?;
    let End::Prop { id: pa, .. } = na else {
        return Err(ScriptError::new("a weld's first end is a prop"));
    };
    let (b_end, b_prop) = match nb {
        End::Prop { id, .. } => (
            End::Prop {
                id,
                local: [0.0; 3],
            },
            Some(id),
        ),
        End::World => (End::World, None),
        _ => {
            return Err(ScriptError::new(
                "a weld's second end is a prop or the world",
            ));
        }
    };
    let frame = props
        .0
        .borrow_mut()
        .world_mut(&vm)
        .weld_frame(Some(pa), b_prop)
        .ok_or_else(|| ScriptError::new(JointError::Bad.to_string()))?;
    let owner = owner_of(&mut vm, owner);
    let a_end = End::Prop {
        id: pa,
        local: [0.0; 3],
    };
    made(
        &mut vm,
        &props,
        Joint::new(Kind::Weld { frame }, a_end, b_end, owner),
    )
}

/// hinges a prop to another (or, with `b` empty, to the world) about `axis` (`"x y z"`) through `point`, and returns the joint's
/// object id
///
/// `jointSetLimits` holds the angle between two limits, `jointSetMotor` turns it.
#[function(name = "jointHinge")]
fn joint_hinge(
    mut vm: VmMut,
    props: Res<ServerProps>,
    a: &str,
    b: &str,
    point: &str,
    axis: &str,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let point = finite(point, "the point")?;
    let axis = finite(axis, "the axis")?;
    let (na, nb) = ends(&mut vm, &props, a, b)?;
    let End::Prop { id: pa, .. } = na else {
        return Err(ScriptError::new("a hinge's first end is a prop"));
    };
    let owner = owner_of(&mut vm, owner);
    let joint = {
        let mut srv = props.0.borrow_mut();
        let world = srv.world_mut(&vm);
        let bad = || ScriptError::new(JointError::Bad.to_string());
        let a_end = End::Prop {
            id: pa,
            local: world.local_of(pa, point).ok_or_else(bad)?,
        };
        let axis_a = world.local_dir_of(pa, axis).ok_or_else(bad)?;
        let (b_end, axis_b) = match nb {
            End::Prop { id, .. } => (
                End::Prop {
                    id,
                    local: world.local_of(id, point).ok_or_else(bad)?,
                },
                world.local_dir_of(id, axis).ok_or_else(bad)?,
            ),
            End::World => (End::Point(point), axis),
            _ => {
                return Err(ScriptError::new(
                    "a hinge's second end is a prop or the world",
                ));
            }
        };
        let hinge = Hinge {
            axis_a,
            axis_b,
            limits: None,
            motor: None,
        };
        Joint::new(Kind::Hinge(hinge), a_end, b_end, owner)
    };
    made(&mut vm, &props, joint)
}

/// Runs `f` on the world and writes back the mask it returns; false when it returned none.
fn change(
    vm: &mut Vm,
    props: &ServerProps,
    id: u32,
    f: impl FnOnce(&mut PropWorld) -> Option<u32>,
) -> bool {
    let mut srv = props.0.borrow_mut();
    let world = srv.world_mut(vm);
    let Some(mask) = f(world) else {
        return false;
    };
    write_back(vm, world, id, mask);
    true
}

/// makes a tether a winch: its length moves to `length` at `rate` units a second (at most 20), and stops shortening while it
/// pulls with more than `maxForce` (left out or 0: no limit); true for a tether
#[function(name = "jointWinch")]
fn joint_winch(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    length: f32,
    rate: f32,
    max_force: Option<f32>,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let winch = Winch {
        target: number(length, "the length")?,
        rate: number(rate, "the rate")?,
        max_force: number(max_force.unwrap_or(0.0), "the force")?,
    };
    Ok(change(&mut vm, &props, id, |w| w.set_winch(id, winch)))
}

/// sets a tether's, a winch's or a grapple's length (0.001 to 200) and, if given, its shortest; true for one of those
#[function(name = "jointSetLength")]
fn joint_set_length(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    length: f32,
    min: Option<f32>,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let length = number(length, "the length")?;
    let min = min.map(|m| number(m, "the shortest")).transpose()?;
    Ok(change(&mut vm, &props, id, |w| {
        w.set_reach(id, |r| {
            r.max = length;
            if let Some(m) = min {
                r.min = m;
            }
        })
    }))
}

/// makes a tether or a grapple a spring past its length (`stiffness` force a unit past it, `damping` force a unit a second);
/// a stiffness of 0 makes it hard again; true for one of those
#[function(name = "jointSetSpring")]
fn joint_set_spring(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    stiffness: f32,
    damping: Option<f32>,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let spring = Spring {
        stiffness: number(stiffness, "the stiffness")?,
        damping: number(damping.unwrap_or(0.0), "the damping")?,
    };
    Ok(change(&mut vm, &props, id, |w| {
        w.set_reach(id, |r| r.spring = Some(spring))
    }))
}

/// sets the force a joint breaks past (a torque counts at one unit; 0: never); true for a joint
#[function(name = "jointSetBreakForce")]
fn joint_set_break_force(
    vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    force: f32,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let force = number(force, "the force")?;
    Ok(props
        .0
        .borrow_mut()
        .world_mut(&vm)
        .set_break_force(id, force))
}

/// holds a hinge's angle between `low` and `high` (radians, from where it was made); with neither given, frees it; true for a
/// hinge
#[function(name = "jointSetLimits")]
fn joint_set_limits(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    low: Option<f32>,
    high: Option<f32>,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let limits = match (low, high) {
        (Some(lo), Some(hi)) => Some([number(lo, "the low limit")?, number(hi, "the high limit")?]),
        (None, None) => None,
        _ => return Err(ScriptError::new("give both limits, or neither")),
    };
    Ok(change(&mut vm, &props, id, |w| {
        w.set_hinge(id, |h| h.limits = limits)
    }))
}

/// turns a hinge at `speed` radians a second with at most `maxTorque` (0: the motor is off); true for a hinge
#[function(name = "jointSetMotor")]
fn joint_set_motor(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    speed: f32,
    max_torque: f32,
) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let motor = Motor {
        speed: number(speed, "the speed")?,
        max_torque: number(max_torque, "the torque")?,
    };
    let motor = (motor.max_torque > 0.0).then_some(motor);
    Ok(change(&mut vm, &props, id, |w| {
        w.set_hinge(id, |h| h.motor = motor)
    }))
}

/// takes a joint away from the world and every client (and the rope that shows it); true if there was one
#[function(name = "jointDelete")]
fn joint_delete(mut vm: VmMut, props: Res<ServerProps>, joint: i32) -> bool {
    let Ok(id) = joint_of(&vm, joint) else {
        return false;
    };
    let gone = props
        .0
        .borrow_mut()
        .world
        .as_mut()
        .and_then(|w| w.remove_joint(id));
    let rope = gone.map_or(0, |j| j.rope);
    super::server::delete_objects(&mut vm, &[(id, rope)]);
    true
}

fn with_joint<R>(props: &ServerProps, id: u32, f: impl FnOnce(&Joint) -> R) -> Option<R> {
    props
        .with(|s| s.world.as_ref().and_then(|w| w.joint(id)).map(f))
        .flatten()
}

/// a joint's kind: `tether`, `winch`, `weld`, `hinge` or `grapple`
#[function(name = "jointGetKind")]
fn joint_get_kind(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<String, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.kind.id().name().to_string()).unwrap_or_default())
}

/// a tether's, a winch's or a grapple's length now (0 for the other kinds)
#[function(name = "jointGetLength")]
fn joint_get_length(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<f32, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.kind.reach().map_or(0.0, |r| r.max)).unwrap_or(0.0))
}

/// the distance between a joint's ends after the last tick
#[function(name = "jointGetDistance")]
fn joint_get_distance(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<f32, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.status.distance).unwrap_or(0.0))
}

/// the force a joint held its ends with in the last tick
#[function(name = "jointGetForce")]
fn joint_get_force(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<f32, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.status.force).unwrap_or(0.0))
}

/// whether a tether's ends are at its length (a rope following it is drawn taut)
#[function(name = "jointIsAtLimit")]
fn joint_is_at_limit(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.status.at_limit).unwrap_or(false))
}

/// whether a joint broke (it holds nothing; delete it when it is done with)
#[function(name = "jointIsBroken")]
fn joint_is_broken(vm: VmMut, props: Res<ServerProps>, joint: i32) -> Result<bool, ScriptError> {
    let id = joint_of(&vm, joint)?;
    Ok(with_joint(&props, id, |j| j.status.broken).unwrap_or(false))
}

/// how many joints there are, or, given a client, how many its owner key owns
#[function(name = "jointCount")]
fn joint_count(vm: VmMut, props: Res<ServerProps>, owner: Option<i32>) -> i32 {
    let key = owner
        .and_then(|o| ObjId::try_from(o).ok())
        .filter(|&o| o != 0 && vm.objects().contains(o))
        .map(|o| owner_key(&vm, o));
    props
        .with(|s| {
            s.world.as_ref().map_or(0, |w| match &key {
                Some(k) => w.joints_owned_by(k) as i32,
                None => w.joints().len() as i32,
            })
        })
        .unwrap_or(0)
}

/// how many joints a prop is an end of
#[function(name = "jointCountOn")]
fn joint_count_on(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<i32, ScriptError> {
    let id = crate::server::prop_of(&vm, prop)?;
    Ok(props
        .with(|s| s.world.as_ref().map_or(0, |w| w.joints_on(id) as i32))
        .unwrap_or(0))
}

/// checks whether two ends could be joined for a player: "" when they may, else the reason
///
/// `client` is the connection asking (the joint's owner). `a` and `b` are ends as `jointTether` takes them (a weld's or a hinge's
/// world end may be empty); `kind` is the joint's (`tether` when left out or empty: `winch`, `weld`, `hinge`, `grapple` and
/// `hold`, whose first end is a player), and a hinge gives its `point`. Every end and the point must be within `reach` of `eye`
/// (the player's eye, as the server gives it: 8 when left out). A player at an end is where the server has it, and must be the
/// client's own control object unless `allowOtherPlayers`; a prop at an end must be the client's (its owner key) or no one's
/// unless `allowOtherOwners` (each a game's choice, off by default). The caps must allow one more joint for the client, its
/// address and each prop, and a player may have one grapple. Joints have no client entry point: a game's own server command
/// (with its declared limits and permission) calls this first.
#[function(name = "jointCheck")]
#[expect(
    clippy::too_many_arguments,
    reason = "a console function's arguments, each a script argument"
)]
fn joint_check(
    mut vm: VmMut,
    props: Res<ServerProps>,
    client: i32,
    a: &str,
    b: &str,
    eye: &str,
    reach: Option<f32>,
    kind: Option<&str>,
    point: Option<&str>,
    allow_other_players: Option<bool>,
    allow_other_owners: Option<bool>,
) -> String {
    let whose = |allowed: Option<bool>| {
        if allowed.unwrap_or(false) {
            Whose::Anyone
        } else {
            Whose::Own
        }
    };
    let mut check = || -> Result<(), JointCheckError> {
        let eye = finite(eye, "the eye").map_err(|_| JointCheckError::Eye)?;
        let client = ObjId::try_from(client)
            .ok()
            .filter(|&c| c != 0 && vm.objects().contains(c))
            .ok_or(JointCheckError::Client)?;
        let kind = match kind.map(str::trim).filter(|k| !k.is_empty()) {
            None => KindId::Tether,
            Some(k) => KindId::named(k).ok_or(JointCheckError::Kind)?,
        };
        let point = match point.map(str::trim).filter(|p| !p.is_empty()) {
            None => None,
            Some(p) => Some(finite(p, "the point").map_err(|_| JointCheckError::Point)?),
        };
        let (a, b) = ends(&mut vm, &props, a, b).map_err(|_| JointCheckError::Ends)?;
        let request = JointCheck {
            client,
            kind,
            a,
            b,
            point,
            eye,
            reach: reach.unwrap_or(DEFAULT_REACH),
            players: whose(allow_other_players),
            props: whose(allow_other_owners),
        };
        props.check_joint(&mut vm, &request)
    };
    check().err().map_or_else(String::new, |e| e.to_string())
}

/// The farthest an end may be from the eye when `jointCheck` names no reach (units).
pub const DEFAULT_REACH: f32 = 8.0;

/// Whose things a joint check lets the asking client join ([`JointCheck`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whose {
    /// Only the client's own: its control object, or a prop it owns (by its owner key) or no one owns.
    #[default]
    Own,
    /// Anyone's (a game's choice).
    Anyone,
}

/// Two ends a game wants to join for a player ([`ServerProps::check_joint`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointCheck {
    /// The connection asking: the joint's owner.
    pub client: ObjId,
    /// The kind of joint: which ends it takes ([`PropWorld::may_join`]).
    pub kind: KindId,
    pub a: End,
    /// [`End::World`] for a weld's or a hinge's world end.
    pub b: End,
    /// A hinge's point in the world: a hinge must give it, and it must be within reach.
    pub point: Option<[f32; 3]>,
    /// The player's eye as the server gives it, never a point the client sent.
    pub eye: [f32; 3],
    /// The farthest an end may be from the eye ([`DEFAULT_REACH`]).
    pub reach: f32,
    /// Whose players may be at an end.
    pub players: Whose,
    /// Whose props may be at an end.
    pub props: Whose,
}

/// Why a joint was refused for a player ([`ServerProps::check_joint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointCheckError {
    /// The eye is not finite.
    Eye,
    /// The client is not a connection there is.
    Client,
    /// The kind is not a joint's.
    Kind,
    /// A hinge gave no point, or one that is not finite.
    Point,
    /// An end is not something there is.
    Ends,
    /// A player at an end is not the client's own control object.
    OtherPlayer,
    /// A prop at an end is owned by someone other than the client.
    OtherOwner,
    /// A player at an end has no position the server can read.
    NoPosition,
    /// An end or the point is farther from the eye than the reach.
    TooFar,
    /// A cap or the ends' rules ([`PropWorld::may_join`]).
    Joint(JointError),
}

impl std::fmt::Display for JointCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JointCheckError::Eye => f.write_str("the eye is out of the world"),
            JointCheckError::Client => f.write_str("there is no such client"),
            JointCheckError::Kind => f.write_str("that is not a kind of joint"),
            JointCheckError::Point => f.write_str("a hinge needs its point"),
            JointCheckError::Ends => JointError::Ends.fmt(f),
            JointCheckError::OtherPlayer => f.write_str("a player may only join itself"),
            JointCheckError::OtherOwner => f.write_str("that prop is someone else's"),
            JointCheckError::NoPosition => f.write_str("that player is nowhere the server can see"),
            JointCheckError::TooFar => f.write_str("that is out of reach"),
            JointCheckError::Joint(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for JointCheckError {}

impl ServerProps {
    /// Whether two ends may be joined for a player: the checks of `jointCheck`. A game calls it from its own server command
    /// before it makes the joint.
    ///
    /// # Errors
    ///
    /// The first check that fails ([`JointCheckError`]): a hinge with no point, a player at an end that is not the client's
    /// control object or a prop owned by someone else (unless allowed), a player whose position cannot be read, an end or the
    /// point out of reach, or the ends' rules and the caps.
    pub fn check_joint(&self, vm: &mut Vm, check: &JointCheck) -> Result<(), JointCheckError> {
        if check.kind == KindId::Hinge && check.point.is_none() {
            return Err(JointCheckError::Point);
        }
        let control = crate::natives::control_of(vm, check.client);
        let owner = server_owner_of(vm, check.client);
        let srv = self.0.borrow();
        let world = srv.world.as_ref().ok_or(JointCheckError::Ends)?;
        let reach = if check.reach > 0.0 {
            check.reach.min(MAX_LENGTH)
        } else {
            0.0
        };
        let in_reach = |at: [f32; 3]| {
            let distance = length(std::array::from_fn(|k| at[k] - check.eye[k]));
            // A distance that is not a number is out of reach.
            if distance.is_nan() || distance > reach {
                Err(JointCheckError::TooFar)
            } else {
                Ok(())
            }
        };
        for end in [&check.a, &check.b] {
            let at = match *end {
                End::World => continue,
                End::Player { id, offset } => {
                    if check.players == Whose::Own && control != Some(id) {
                        return Err(JointCheckError::OtherPlayer);
                    }
                    let p = torque_game::sim::player::position(vm, id)
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .ok_or(JointCheckError::NoPosition)?;
                    std::array::from_fn(|k| p[k] + offset[k])
                }
                End::Prop { id, .. } => {
                    let theirs = world
                        .get(id)
                        .and_then(|p| p.owner.as_ref())
                        .is_some_and(|o| o.key != owner.key);
                    if check.props == Whose::Own && theirs {
                        return Err(JointCheckError::OtherOwner);
                    }
                    world.end_point(end).ok_or(JointCheckError::Ends)?
                }
                End::Point(_) => world.end_point(end).ok_or(JointCheckError::Ends)?,
            };
            in_reach(at)?;
        }
        if let Some(point) = check.point {
            in_reach(point)?;
        }
        // A hinge to the world holds at its point: the end the world takes for it.
        let b = match (check.kind, check.b, check.point) {
            (KindId::Hinge, End::World, Some(point)) => End::Point(point),
            (_, b, _) => b,
        };
        world
            .may_join(check.kind, &check.a, &b, Some(&owner))
            .map_err(JointCheckError::Joint)
    }
}

/// Whether `id` is a joint.
pub fn is_joint(vm: &Vm, id: u32) -> bool {
    is_class(vm, id, CLASS)
}

/// The natives, as the plugin registers them.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![
        joint_tether,
        joint_grapple,
        joint_hold,
        joint_weld,
        joint_hinge,
        joint_winch,
        joint_set_length,
        joint_set_spring,
        joint_set_break_force,
        joint_set_limits,
        joint_set_motor,
        joint_delete,
        joint_get_kind,
        joint_get_length,
        joint_get_distance,
        joint_get_force,
        joint_is_at_limit,
        joint_is_broken,
        joint_count,
        joint_count_on,
        joint_check
    ]
}
