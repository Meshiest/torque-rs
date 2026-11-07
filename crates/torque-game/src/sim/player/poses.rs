//! The engine's poses on the host (after Torque3D's; the step's half is `::sim::player::pose`): the datablock's pose fields, each
//! pose's own actions and how a Player's action is picked in a pose. The server and a client pick by the same rules
//! ([`pick_in`]), so a synced Player's hitboxes stay the pose the clients draw.
//!
//! - **The datablock** ([`pose_data`]): Torque3D's fields, read as dynamic fields of the `PlayerData` (TGE's class has none of
//!   them): `crouchBoundingBox` with a shape that has `crouch_root` allows crouching, `proneBoundingBox` with `prone_root`
//!   lying down, and `maxSprintForwardSpeed` sprinting; the speeds (`maxCrouchForwardSpeed`, `maxCrouchBackwardSpeed`,
//!   `maxCrouchSideSpeed`, the same for prone and sprint), the forces (`crouchForce`, `proneForce`, `sprintForce`) and
//!   `sprintCanJump`. The triggers are Torque3D's globals `$player::crouchTrigger`, `$player::proneTrigger` and
//!   `$player::sprintTrigger` (3, 4 and 5 unset). `swimBoundingBox` allows swimming, with `swimForce`: a shape without
//!   `swim_root` swims with the standing actions.
//! - **The actions** ([`PoseSets`]): a pose's `<pose>_root`, `<pose>_forward`, `<pose>_backward` and `<pose>_side` (a step to
//!   the left, played backward for a step to the right), as Torque3D names them (`crouch_root`, `prone_forward`, ...). A pose
//!   whose shape has no `<pose>_root` plays the standing actions (sprinting, unless the shape has `sprint_root`). Torque3D's
//!   `<pose>_left` and `<pose>_right` (its swimming's `swim_left`, `swim_right`) are taken for the side step when the shape has
//!   no `<pose>_side`: the one step, played backward the other way, as the side step is.
//! - **The pick** ([`pick_in`]): falling plays the table's `fall` in any pose; otherwise the pose's moving action whose ground
//!   direction best fits the velocity past 0.1 (`side` also backwards), else its root, as TGE picks the standing ones.
//! - **The way down and up** ([`PoseSets::way`]): a shape with `prone_down` plays it, to its end, when the Player goes prone
//!   from the crouch (or from standing), and `prone_up` when it gets up from prone: the body lowers to a knee, puts its
//!   hands down and lies flat, or pushes up to the crouch, in the ticks the step keeps the Player still for it. A shape may
//!   give a way of its own for a Player whose hands are not empty (`prone_down_armed`, `prone_up_armed`: its arm action is not
//!   the shape's `look`, a pistol held ahead staying ahead), which then plays in its place.
//! - **The blend** ([`blend_for`]): an action of another pose blends in over a longer, eased transition, so the body lowers
//!   and rises rather than snapping: [`CROUCH_BLEND`] seconds between standing and crouching, [`PRONE_BLEND`] to or from prone
//!   (without a way down or up), [`SWIM_BLEND`] into or out of swimming, [`WAY_BLEND`] into a way down or up from the pose it
//!   starts in.
//! - **The arm** ([`pose_arm`]): an arm action plays the version of the action playing when the shape has one
//!   (`<action>_<arm>`: `swim_forward_point` while `swim_forward` plays), else the pose's (`<pose>_<arm>`: `prone_hold` lying
//!   down, `swim_point` swimming), else itself.
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
/// The seconds a change to or from prone blends over when the shape has no way down or up of its own (`prone_down`,
/// `prone_up`).
pub const PRONE_BLEND: f32 = 0.55;
/// The seconds getting down from the crouch to prone and pushing up again take (the step's ticks, `::sim`'s
/// `PRONE_DOWN_TICKS` and `PRONE_UP_TICKS`): the eye's ease between the two.
pub const PRONE_DOWN_SECONDS: f32 =
    ::sim::player::pose::PRONE_DOWN_TICKS as f32 * ::sim::player::DT;
pub const PRONE_UP_SECONDS: f32 = ::sim::player::pose::PRONE_UP_TICKS as f32 * ::sim::player::DT;
/// The seconds a way down or up (`prone_down`, `prone_up`) blends in over from the crouch or prone it starts in.
pub const WAY_BLEND: f32 = 0.12;
/// The seconds a change into or out of swimming blends over: the body leans into the stroke or stands up out of it.
pub const SWIM_BLEND: f32 = 0.4;
/// The largest size of a pose's box a datablock may give (each of width, depth and height, m).
pub const MAX_POSE_BOX: f32 = 100.0;
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
    // A pose's box: three finite sizes over 0 and at most `MAX_POSE_BOX` (a datablock is not trusted: an infinite box makes
    // the room test's query infinite).
    let sized = |name: &[u8], out: &mut [f32; 3]| {
        floats(name, out) && out.iter().all(|v| *v > 0.0 && *v <= MAX_POSE_BOX)
    };
    let mut b = [0.0f32; 3];
    if sized(b"crouchBoundingBox", &mut b) && has(b"crouch_root") {
        d.crouch = true;
        d.crouch_box = b;
    }
    if sized(b"proneBoundingBox", &mut b) && has(b"prone_root") {
        d.prone = true;
        d.prone_box = b;
    }
    if sized(b"swimBoundingBox", &mut b) {
        d.swim = true;
        d.swim_box = b;
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
    float(b"swimForce", &mut d.swim_force);
    d.swim_force = d.swim_force.max(0.0);
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
    /// The way into the pose from the crouch (`<pose>_down`), and out of it up to the crouch (`<pose>_up`).
    pub down: Option<usize>,
    pub up: Option<usize>,
    /// The ways for a Player whose hands are not empty (`<pose>_down_armed`, `<pose>_up_armed`), when the shape has them.
    pub down_armed: Option<usize>,
    pub up_armed: Option<usize>,
}

impl PoseActions {
    /// The moving actions: forward, backward, side.
    pub fn moves(&self) -> [Option<usize>; 3] {
        [self.forward, self.backward, self.side]
    }

    fn contains(&self, action: usize) -> bool {
        self.root == Some(action)
            || self.down == Some(action)
            || self.down_armed == Some(action)
            || self.moves().contains(&Some(action))
    }
}

/// Every pose's actions (standing's are the table's, so its set is empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoseSets {
    sets: [PoseActions; 5],
}

fn index(pose: Pose) -> usize {
    match pose {
        Pose::Stand => 0,
        Pose::Sprint => 1,
        Pose::Crouch => 2,
        Pose::Prone => 3,
        Pose::Swim => 4,
    }
}

impl PoseSets {
    /// The sets of an action list (`name`, whether it has a sequence, and its ground motion, in the list's order): each
    /// pose's root by name, and its moving actions by name when they have a sequence that moves.
    pub fn of<'a>(
        actions: impl IntoIterator<Item = (&'a [u8], bool, &'a ActionMotion)>,
    ) -> PoseSets {
        let mut sets = PoseSets::default();
        // Each pose's first `<pose>_left` or `<pose>_right`, its side step when it has no `<pose>_side`.
        let mut steps = [None; 5];
        for (i, (name, has, motion)) in actions.into_iter().enumerate() {
            if !has {
                continue;
            }
            for pose in [Pose::Sprint, Pose::Crouch, Pose::Prone, Pose::Swim] {
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
                    b"left" | b"right" if moves => {
                        steps[index(pose)] = steps[index(pose)].or(Some(i));
                    }
                    b"down" => set.down = set.down.or(Some(i)),
                    b"up" => set.up = set.up.or(Some(i)),
                    b"down_armed" => set.down_armed = set.down_armed.or(Some(i)),
                    b"up_armed" => set.up_armed = set.up_armed.or(Some(i)),
                    _ => {}
                }
            }
        }
        for (set, step) in sets.sets.iter_mut().zip(steps) {
            set.side = set.side.or(step);
        }
        sets
    }

    /// The actions of `pose`, when its shape has its root (else the standing ones play).
    pub fn of_pose(&self, pose: Pose) -> Option<&PoseActions> {
        let set = &self.sets[index(pose)];
        set.root.is_some().then_some(set)
    }

    /// The pose an action belongs to (standing for the table's and every other; a way down to the pose it lies down in, a way
    /// up to the crouch it rises to).
    pub fn pose_of(&self, action: usize) -> Pose {
        if self
            .sets
            .iter()
            .any(|s| s.up == Some(action) || s.up_armed == Some(action))
        {
            return Pose::Crouch;
        }
        [Pose::Sprint, Pose::Crouch, Pose::Prone, Pose::Swim]
            .into_iter()
            .find(|p| self.sets[index(*p)].contains(action))
            .unwrap_or(Pose::Stand)
    }

    /// Whether the action is a pose's moving one (its rate follows the ground covered, as `run`'s does).
    pub fn is_moving(&self, action: usize) -> bool {
        self.sets.iter().any(|s| s.moves().contains(&Some(action)))
    }

    /// Whether the action is a way down or up.
    pub fn is_way(&self, action: usize) -> bool {
        self.sets
            .iter()
            .any(|s| [s.down, s.up, s.down_armed, s.up_armed].contains(&Some(action)))
    }

    /// The way down or up to play before the actions of `pose` (see the module docs), the action playing being `now`: prone's
    /// way down when it is not lying down yet, its way up when it lies down and the pose is not prone; none otherwise, or when
    /// the shape has no such way. `armed`: the Player's hands are not empty, so the shape's armed way plays when it has one.
    pub fn way(&self, now: usize, pose: Pose, armed: bool) -> Option<usize> {
        let prone = self.of_pose(Pose::Prone)?;
        let lying = self.pose_of(now) == Pose::Prone;
        let pick = |plain: Option<usize>, held: Option<usize>| {
            if armed { held.or(plain) } else { plain }
        };
        match (pose == Pose::Prone, lying) {
            (true, false) => pick(prone.down, prone.down_armed),
            (false, true) => pick(prone.up, prone.up_armed),
            _ => None,
        }
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

/// The seconds a change from pose `from` to pose `to` eases over; none when the two are the same pose (the usual blend). Between
/// the crouch and prone it is the time the body takes to get down or up (the eye's ease follows it).
pub fn pose_blend(from: Pose, to: Pose) -> Option<f32> {
    if swim_change(from, to) {
        return Some(SWIM_BLEND);
    }
    match (from.lowness(), to.lowness()) {
        (a, b) if a == b => None,
        (1, 2) => Some(PRONE_DOWN_SECONDS),
        (2, 1) => Some(PRONE_UP_SECONDS),
        (a, b) if a == 2 || b == 2 => Some(PRONE_BLEND),
        _ => Some(CROUCH_BLEND),
    }
}

/// The seconds the action `to` blends in over from the action `from`, eased (none: the usual blend): a way down or up blends
/// in quickly from the pose it starts in, and the pose after it as usual; another change of pose over its own blend, prone's
/// without a way down or up over [`PRONE_BLEND`].
pub fn blend_for(sets: &PoseSets, from: usize, to: usize) -> Option<f32> {
    if sets.is_way(to) {
        return Some(WAY_BLEND);
    }
    if sets.is_way(from) {
        return None;
    }
    let (a, b) = (sets.pose_of(from), sets.pose_of(to));
    if swim_change(a, b) {
        return Some(SWIM_BLEND);
    }
    match (a.lowness(), b.lowness()) {
        (x, y) if x == y => None,
        (x, y) if x == 2 || y == 2 => Some(PRONE_BLEND),
        _ => Some(CROUCH_BLEND),
    }
}

/// Whether a change from pose `from` to `to` goes into or out of swimming (swimming is as low as standing, so the lowness
/// alone does not tell).
fn swim_change(from: Pose, to: Pose) -> bool {
    from != to && (from == Pose::Swim || to == Pose::Swim)
}

/// Whether a Player whose arm action is named `arm` has its hands full (for [`PoseSets::way`]): any arm action but `look`,
/// the empty hands a Player's arm thread starts on.
pub fn armed(arm: Option<&[u8]>) -> bool {
    arm.is_some_and(|a| !a.eq_ignore_ascii_case(b"look"))
}

/// The version of an arm action `arm` for a Player in `pose` playing the action named `action`, in the action list `names`
/// (see the module docs): the action's own (`<action>_<arm>`) when the shape has one, else the pose's (`<pose>_<arm>`,
/// `prone_hold` for `hold` lying down); none otherwise (the arm action itself plays). Both sides play it, so the arm the clients
/// draw is the one hits are cast at.
pub fn pose_arm<'a>(
    names: impl IntoIterator<Item = (&'a [u8], bool)>,
    arm: &[u8],
    pose: Pose,
    action: &[u8],
) -> Option<usize> {
    let named = |name: &[u8], prefix: &[u8]| {
        name.len() == prefix.len() + 1 + arm.len()
            && name[..prefix.len()].eq_ignore_ascii_case(prefix)
            && name[prefix.len()] == b'_'
            && name[prefix.len() + 1..].eq_ignore_ascii_case(arm)
    };
    let prefix = pose.action_prefix().map(str::as_bytes);
    let (mut own, mut posed) = (None, None);
    for (i, (name, has)) in names.into_iter().enumerate() {
        if !has {
            continue;
        }
        if own.is_none() && !action.is_empty() && named(name, action) {
            own = Some(i);
        }
        if posed.is_none() && prefix.is_some_and(|p| named(name, p)) {
            posed = Some(i);
        }
    }
    own.or(posed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The poses of a PlayerData whose fields are `fields` (script text), its shape having every pose's root.
    fn poses_of(fields: &str) -> PoseData {
        let mut vm = Vm::new();
        crate::specs::install(&mut vm);
        let text = format!("datablock PlayerData(Posed) {{ {fields} }}; return Posed.getId();");
        let id = vm.eval(text.as_bytes()).expect("the datablock");
        let id: ObjId = String::from_utf8(id).unwrap().parse().unwrap();
        pose_data(&vm, id, |_| true)
    }

    /// A pose's box must be three finite sizes over 0 and at most [`MAX_POSE_BOX`], or the datablock does not allow the
    /// pose; the swim force is not negative.
    #[test]
    fn a_hostile_datablock_s_boxes_and_swim_force_are_refused() {
        let good = poses_of(
            r#"crouchBoundingBox = "1 1 1.6"; proneBoundingBox = "2 2 0.8"; swimBoundingBox = "1 2.3 1"; swimForce = 900;"#,
        );
        assert!(good.crouch && good.prone && good.swim);
        assert_eq!((good.swim_box, good.swim_force), ([1.0, 2.3, 1.0], 900.0));
        for bad in ["inf 1 1", "1 nan 1", "0 1 1", "1 -2 1", "1 1 1e9", "1 1"] {
            let d = poses_of(&format!(
                r#"crouchBoundingBox = "{bad}"; proneBoundingBox = "{bad}"; swimBoundingBox = "{bad}";"#
            ));
            assert!(!d.crouch && !d.prone && !d.swim, "{bad}: {d:?}");
        }
        let d = poses_of(r#"swimBoundingBox = "1 2.3 1"; swimForce = -500;"#);
        assert_eq!(d.swim_force, 0.0);
        let d = poses_of(r#"swimBoundingBox = "1 2.3 1"; swimForce = "inf";"#);
        assert_eq!(d.swim_force, PoseData::default().swim_force);
    }

    fn moving(speed: f32) -> ActionMotion {
        ActionMotion {
            dir: [0.0, 1.0, 0.0],
            speed,
            velocity_scale: false,
            death: false,
        }
    }

    /// Torque3D's `swim_left` and `swim_right` are the side step when the shape has no `swim_side`; `swim_side` wins when it
    /// has one.
    #[test]
    fn a_pose_s_left_or_right_step_is_its_side_step_when_it_has_no_side() {
        let (still, go) = (moving(0.0), moving(2.0));
        let names: [&[u8]; 4] = [b"swim_root", b"swim_forward", b"swim_right", b"swim_left"];
        let list = names
            .iter()
            .map(|n| (*n, true, if n.ends_with(b"root") { &still } else { &go }));
        let sets = PoseSets::of(list);
        let swim = sets.of_pose(Pose::Swim).unwrap();
        assert_eq!(
            (swim.root, swim.forward, swim.side),
            (Some(0), Some(1), Some(2))
        );
        let names: [&[u8]; 3] = [b"swim_root", b"swim_left", b"swim_side"];
        let list = names
            .iter()
            .map(|n| (*n, true, if n.ends_with(b"root") { &still } else { &go }));
        assert_eq!(
            PoseSets::of(list).of_pose(Pose::Swim).unwrap().side,
            Some(2)
        );
    }

    /// Into or out of swimming the body blends over [`SWIM_BLEND`], though swimming is as low as standing.
    #[test]
    fn going_into_or_out_of_the_water_blends_the_body() {
        for other in [Pose::Stand, Pose::Sprint, Pose::Crouch, Pose::Prone] {
            assert_eq!(pose_blend(other, Pose::Swim), Some(SWIM_BLEND));
            assert_eq!(pose_blend(Pose::Swim, other), Some(SWIM_BLEND));
        }
        assert_eq!(pose_blend(Pose::Swim, Pose::Swim), None);
        assert_eq!(pose_blend(Pose::Stand, Pose::Sprint), None);
        let (still, go) = (moving(0.0), moving(3.0));
        let names: [&[u8]; 3] = [b"root", b"swim_root", b"swim_forward"];
        let list = names
            .iter()
            .map(|n| (*n, true, if *n == b"swim_forward" { &go } else { &still }));
        let sets = PoseSets::of(list);
        assert_eq!(blend_for(&sets, 0, 1), Some(SWIM_BLEND));
        assert_eq!(blend_for(&sets, 2, 0), Some(SWIM_BLEND));
        assert_eq!(
            blend_for(&sets, 1, 2),
            None,
            "within swimming, the usual blend"
        );
    }

    /// The arm action plays the action's own version, else the pose's, else itself; one the shape has no sequence for is
    /// passed over.
    #[test]
    fn an_arm_action_plays_the_action_s_version_then_the_pose_s() {
        let names: [(&[u8], bool); 6] = [
            (b"point", true),
            (b"hold", true),
            (b"swim_point", true),
            (b"swim_forward_point", true),
            (b"swim_hold", true),
            (b"swim_forward_hold", false),
        ];
        let arm = |arm: &[u8], pose: Pose, action: &[u8]| pose_arm(names, arm, pose, action);
        assert_eq!(arm(b"point", Pose::Swim, b"swim_root"), Some(2));
        assert_eq!(arm(b"point", Pose::Swim, b"swim_forward"), Some(3));
        assert_eq!(
            arm(b"POINT", Pose::Swim, b"Swim_Forward"),
            Some(3),
            "any case"
        );
        assert_eq!(
            arm(b"hold", Pose::Swim, b"swim_forward"),
            Some(4),
            "no sequence: the pose's"
        );
        assert_eq!(
            arm(b"look", Pose::Swim, b"swim_forward"),
            None,
            "empty hands: no raised arm"
        );
        assert_eq!(arm(b"point", Pose::Stand, b"run"), None);
        assert_eq!(arm(b"point", Pose::Crouch, b"crouch_root"), None);
    }
}
