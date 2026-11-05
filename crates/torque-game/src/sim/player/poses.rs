//! The engine's poses on the host (after Torque3D's; the step's half is `::sim::player::pose`): the datablock's pose fields, each
//! pose's own actions and how a Player's action is picked in a pose. The server and a client pick by the same rules
//! ([`pick_in`]), so a synced Player's hitboxes stay the pose the clients draw.
//!
//! - **The datablock** ([`pose_data`]): Torque3D's fields, read as dynamic fields of the `PlayerData` (TGE's class has none of
//!   them): `crouchBoundingBox` with a shape that has `crouch_root` allows crouching, `proneBoundingBox` with `prone_root`
//!   lying down, and `maxSprintForwardSpeed` sprinting; the speeds (`maxCrouchForwardSpeed`, `maxCrouchBackwardSpeed`,
//!   `maxCrouchSideSpeed`, the same for prone and sprint), the forces (`crouchForce`, `proneForce`, `sprintForce`) and
//!   `sprintCanJump`. The triggers are Torque3D's globals `$player::crouchTrigger`, `$player::proneTrigger` and
//!   `$player::sprintTrigger` (3, 4 and 5 unset).
//! - **The actions** ([`PoseSets`]): a pose's `<pose>_root`, `<pose>_forward`, `<pose>_backward` and `<pose>_side` (a step to
//!   the left, played backward for a step to the right), as Torque3D names them (`crouch_root`, `prone_forward`, ...). A pose
//!   whose shape has no `<pose>_root` plays the standing actions (sprinting, unless the shape has `sprint_root`).
//! - **The pick** ([`pick_in`]): falling plays the table's `fall` in any pose; otherwise the pose's moving action whose ground
//!   direction best fits the velocity past 0.1 (`side` also backwards), else its root, as TGE picks the standing ones.
//! - **The blend** ([`pose_blend`]): an action of another pose blends in over a longer, eased transition, so the body lowers
//!   and rises rather than snapping: [`CROUCH_BLEND`] seconds between standing and crouching, [`PRONE_BLEND`] to or from prone.
//!
//! Credit: the names and the choice follow `PlayerData::ActionAnimationList` and `Player::pickActionAnimation` in Torque3D's
//! MIT-licensed `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is
//! this crate's own.

pub use ::sim::player::{Pose, PoseData, PoseSpeeds, PoseTriggers};
use collide::math::F3;
use ts_vm::sim::ObjId;
use ts_vm::text::{atoi, scan_floats};
use ts_vm::vm::Vm;

use super::ground::ActionMotion;

/// The seconds a change between standing and crouching blends over.
pub const CROUCH_BLEND: f32 = 0.3;
/// The seconds a change to or from prone blends over.
pub const PRONE_BLEND: f32 = 0.55;
/// The smallest match a moving action must beat to be chosen over the root (TGE's 0.1).
pub const MIN_MATCH: f32 = 0.1;

/// The globals that name the pose triggers (Torque3D's).
pub const CROUCH_TRIGGER: &str = "$player::crouchTrigger";
pub const PRONE_TRIGGER: &str = "$player::proneTrigger";
pub const SPRINT_TRIGGER: &str = "$player::sprintTrigger";

/// A dynamic field's text ("" when unset).
fn text(vm: &Vm, db: ObjId, name: &[u8]) -> Vec<u8> {
    let objects = vm.objects();
    objects
        .get(db)
        .map(|o| o.data_field(&objects.classes, name, b""))
        .unwrap_or_default()
}

/// A trigger global: its number when it is one of the move's six, else `default`.
fn trigger(vm: &Vm, name: &str, default: usize) -> usize {
    vm.global(name)
        .filter(|v| !v.is_empty())
        .and_then(|v| usize::try_from(atoi(&v)).ok())
        .filter(|t| *t < ::sim::MAX_TRIGGERS)
        .unwrap_or(default)
}

/// The poses of PlayerData `db` (see the module docs); `has` says whether the shape has a sequence of that name.
pub fn pose_data(vm: &Vm, db: ObjId, has: impl Fn(&[u8]) -> bool) -> PoseData {
    let mut d = PoseData::default();
    let floats = |name: &[u8], out: &mut [f32]| -> bool {
        let t = text(vm, db, name);
        !t.is_empty() && scan_floats(&t, out) as usize == out.len()
    };
    let float = |name: &[u8], v: &mut f32| {
        let mut x = [0.0f32];
        if floats(name, &mut x) && x[0].is_finite() {
            *v = x[0];
        }
    };
    let mut b = [0.0f32; 3];
    if floats(b"crouchBoundingBox", &mut b) && has(b"crouch_root") {
        d.crouch = true;
        d.crouch_box = b;
    }
    if floats(b"proneBoundingBox", &mut b) && has(b"prone_root") {
        d.prone = true;
        d.prone_box = b;
    }
    d.sprint = !text(vm, db, b"maxSprintForwardSpeed").is_empty();
    let speeds = |p: &mut PoseSpeeds, names: [&[u8]; 3]| {
        float(names[0], &mut p.forward);
        float(names[1], &mut p.backward);
        float(names[2], &mut p.side);
    };
    speeds(
        &mut d.crouch_speeds,
        [
            b"maxCrouchForwardSpeed",
            b"maxCrouchBackwardSpeed",
            b"maxCrouchSideSpeed",
        ],
    );
    speeds(
        &mut d.prone_speeds,
        [
            b"maxProneForwardSpeed",
            b"maxProneBackwardSpeed",
            b"maxProneSideSpeed",
        ],
    );
    speeds(
        &mut d.sprint_speeds,
        [
            b"maxSprintForwardSpeed",
            b"maxSprintBackwardSpeed",
            b"maxSprintSideSpeed",
        ],
    );
    float(b"crouchForce", &mut d.crouch_force);
    float(b"proneForce", &mut d.prone_force);
    float(b"sprintForce", &mut d.sprint_force);
    let jump = text(vm, db, b"sprintCanJump");
    if !jump.is_empty() {
        d.sprint_can_jump = ts_vm::fields::atob(&jump);
    }
    let t = PoseTriggers::default();
    d.triggers = PoseTriggers {
        crouch: trigger(vm, CROUCH_TRIGGER, t.crouch),
        prone: trigger(vm, PRONE_TRIGGER, t.prone),
        sprint: trigger(vm, SPRINT_TRIGGER, t.sprint),
    };
    d
}

/// One pose's own actions, by their numbers in the action list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoseActions {
    pub root: Option<usize>,
    pub forward: Option<usize>,
    pub backward: Option<usize>,
    /// A step to the left (played backward, to the right).
    pub side: Option<usize>,
}

impl PoseActions {
    /// The moving actions: forward, backward, side.
    pub fn moves(&self) -> [Option<usize>; 3] {
        [self.forward, self.backward, self.side]
    }

    fn contains(&self, action: usize) -> bool {
        self.root == Some(action) || self.moves().contains(&Some(action))
    }
}

/// Every pose's actions (standing's are the table's, so its set is empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoseSets {
    sets: [PoseActions; 4],
}

fn index(pose: Pose) -> usize {
    match pose {
        Pose::Stand => 0,
        Pose::Sprint => 1,
        Pose::Crouch => 2,
        Pose::Prone => 3,
    }
}

impl PoseSets {
    /// The sets of an action list (`name`, whether it has a sequence, and its ground motion, in the list's order): each
    /// pose's root by name, and its moving actions by name when they have a sequence that moves.
    pub fn of<'a>(
        actions: impl IntoIterator<Item = (&'a [u8], bool, &'a ActionMotion)>,
    ) -> PoseSets {
        let mut sets = PoseSets::default();
        for (i, (name, has, motion)) in actions.into_iter().enumerate() {
            if !has {
                continue;
            }
            for pose in [Pose::Sprint, Pose::Crouch, Pose::Prone] {
                let Some(prefix) = pose.action_prefix() else {
                    continue;
                };
                let Some(rest) = name
                    .strip_prefix(prefix.as_bytes())
                    .and_then(|r| r.strip_prefix(b"_"))
                else {
                    continue;
                };
                let set = &mut sets.sets[index(pose)];
                let moves = motion.speed > 0.0;
                let rest = rest.to_ascii_lowercase();
                match rest.as_slice() {
                    b"root" => set.root = set.root.or(Some(i)),
                    b"forward" if moves => set.forward = set.forward.or(Some(i)),
                    b"backward" if moves => set.backward = set.backward.or(Some(i)),
                    b"side" if moves => set.side = set.side.or(Some(i)),
                    _ => {}
                }
            }
        }
        sets
    }

    /// The actions of `pose`, when its shape has its root (else the standing ones play).
    pub fn of_pose(&self, pose: Pose) -> Option<&PoseActions> {
        let set = &self.sets[index(pose)];
        set.root.is_some().then_some(set)
    }

    /// The pose an action belongs to (standing for the table's and every other).
    pub fn pose_of(&self, action: usize) -> Pose {
        [Pose::Sprint, Pose::Crouch, Pose::Prone]
            .into_iter()
            .find(|p| self.sets[index(*p)].contains(action))
            .unwrap_or(Pose::Stand)
    }

    /// Whether the action is a pose's moving one (its rate follows the ground covered, as `run`'s does).
    pub fn is_moving(&self, action: usize) -> bool {
        self.sets.iter().any(|s| s.moves().contains(&Some(action)))
    }

    /// The pose's action that plays when the Player stands still in it (its root), if the shape has the pose.
    pub fn root(&self, pose: Pose) -> Option<usize> {
        self.of_pose(pose).and_then(|s| s.root)
    }
}

/// The pick within a pose's actions `set` (not falling): the moving action whose ground direction (`dir_of`) best fits the
/// velocity `v` in the Player's frame past [`MIN_MATCH`], the side step also backwards, else the root; with whether it plays
/// forward.
pub fn pick_in(set: &PoseActions, v: F3, dir_of: impl Fn(usize) -> F3) -> Option<(usize, bool)> {
    let root = set.root?;
    let (mut best, mut chosen) = (MIN_MATCH, (root, true));
    for (k, action) in set.moves().into_iter().enumerate() {
        let Some(a) = action else {
            continue;
        };
        let d = dir_of(a);
        let dot = v[0] * d[0] + v[1] * d[1] + v[2] * d[2];
        if dot > best {
            (best, chosen) = (dot, (a, true));
        } else if k == 2 && -dot > best {
            (best, chosen) = (-dot, (a, false));
        }
    }
    Some(chosen)
}

/// The seconds a change from an action of pose `from` to one of pose `to` blends over, eased; none when the two are the same
/// pose (the usual blend).
pub fn pose_blend(from: Pose, to: Pose) -> Option<f32> {
    let lowness = |p: Pose| match p {
        Pose::Stand | Pose::Sprint => 0,
        Pose::Crouch => 1,
        Pose::Prone => 2,
    };
    match (lowness(from), lowness(to)) {
        (a, b) if a == b => None,
        (a, b) if a == 2 || b == 2 => Some(PRONE_BLEND),
        _ => Some(CROUCH_BLEND),
    }
}

/// The pose's own version of an arm action (`<pose>_<arm>`, `prone_hold` for `hold` lying down) in the action list `names`,
/// when the shape has one; the arm action itself otherwise. Both sides play it, so the arm the clients draw is the one hits
/// are cast at.
pub fn pose_arm<'a>(
    names: impl IntoIterator<Item = (&'a [u8], bool)>,
    arm: &[u8],
    pose: Pose,
) -> Option<usize> {
    let prefix = pose.action_prefix()?;
    names.into_iter().position(|(name, has)| {
        has && name.len() == prefix.len() + 1 + arm.len()
            && name[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            && name[prefix.len()] == b'_'
            && name[prefix.len() + 1..].eq_ignore_ascii_case(arm)
    })
}
