//! Poses, after Torque3D's Player (TGE's has none): standing, sprinting, crouched and prone, each with its own collision box
//! and speeds.
//!
//! - **The pose** ([`Pose`]) is picked each `updateMove` from the move's pose triggers ([`PoseTriggers`]: crouch 3, prone 4 and
//!   sprint 5 by default, Torque3D's `$player::crouchTrigger`, `$player::proneTrigger` and `$player::sprintTrigger`), in
//!   Torque3D's order: prone when its trigger is held and the Player may lie down, else crouched, else sprinting, else standing
//!   when there is room to; a Player that has no room to stand stays as it is (from prone it crouches when there is room
//!   for that, the engine's own step). A pose a datablock does not allow ([`PoseData`]) is never picked.
//! - **By way of the crouch** (the engine's own): between standing and prone the Player crouches first, for [`KNEEL_TICKS`]
//!   ticks, when it may, so its body kneels on the way down and up instead of tipping over like a plank; the clients draw the
//!   crouch the server shows them, at the same ticks.
//! - **The room**: a change to a taller pose (or to prone, whose box is longer) needs the new box clear of the static world (the
//!   terrain, interiors and static shapes, not other Players): its sides pulled in by [`ROOM_INSET`] and its bottom raised by
//!   [`ROOM_FLOOR`] so the floor and walls the Player stands against do not count. Under a low ceiling a crouched Player stays
//!   crouched. Torque3D asks the same of its `crouchBoundingBox` and `boundingBox` (`canCrouch`, `canStand`).
//! - **The box**: the object box becomes the pose's (`boundingBox`, `crouchBoundingBox` or `proneBoundingBox`): half its
//!   width and depth each side of the origin, from the feet up its height, as `Player::setPose` makes it.
//! - **The speeds** ([`super::update_move::move_speed`]): the pose's forward, backward and side speeds
//!   (`maxCrouchForwardSpeed` and the rest), the underwater ones first as before; the run force is the pose's (`sprintForce`,
//!   `crouchForce`, `proneForce`).
//! - **Who may**: a Player in its move state, enabled and not mounted; crouching and lying down also on the ground (not
//!   falling). A mounted Player stands.
//!
//! A datablock that allows no pose (the default, [`PoseData::default`]) leaves every step as TGE's.
//!
//! Credit: this follows `Player::setPose`, `Player::canCrouch`, `Player::canStand`, `Player::canProne` and `updateMove`'s pose
//! choice in Torque3D's MIT-licensed `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.), with its datablock fields' names. The code here is this crate's own.

use torque_collide::math::{Box3, F3};
use torque_collide::{ClippedPolyList, Exhausted, WorkingSet, types};

use super::contact::box_planes;
use super::data::PlayerData;
use super::host::PlayerHost;
use super::{Player, PlayerState, net_mask};
use crate::Move;

/// How far a pose's box is pulled in from each side for the room test, so a wall the Player stands against does not count.
pub const ROOM_INSET: f32 = 0.02;
/// How far above the feet the room test starts, so the floor under the Player does not count.
pub const ROOM_FLOOR: f32 = 0.05;
/// The ticks a Player stays crouched on its way between standing and prone (about the crouch's own blend).
pub const KNEEL_TICKS: u32 = 10;

/// A Player's pose (Torque3D's `Player::Pose`, without swimming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Pose {
    #[default]
    Stand,
    Sprint,
    Crouch,
    Prone,
}

impl Pose {
    /// Every pose, in Torque3D's order.
    pub const ALL: [Pose; 4] = [Pose::Stand, Pose::Sprint, Pose::Crouch, Pose::Prone];

    /// The pose's name, as Torque3D's `getPose` gives it.
    pub fn name(self) -> &'static str {
        match self {
            Pose::Stand => "Stand",
            Pose::Sprint => "Sprint",
            Pose::Crouch => "Crouch",
            Pose::Prone => "Prone",
        }
    }

    /// The pose of a name (any case).
    pub fn from_name(name: &[u8]) -> Option<Pose> {
        Pose::ALL
            .into_iter()
            .find(|p| p.name().as_bytes().eq_ignore_ascii_case(name))
    }

    /// The prefix of the pose's own actions (`crouch_root`, `prone_forward`, ...); none for standing.
    pub fn action_prefix(self) -> Option<&'static str> {
        match self {
            Pose::Stand => None,
            Pose::Sprint => Some("sprint"),
            Pose::Crouch => Some("crouch"),
            Pose::Prone => Some("prone"),
        }
    }

    /// Whether the pose is lower than standing (its box is not the bounding box).
    pub fn is_low(self) -> bool {
        matches!(self, Pose::Crouch | Pose::Prone)
    }

    /// How low the pose is: standing and sprinting 0, crouched 1, prone 2.
    pub fn lowness(self) -> i32 {
        match self {
            Pose::Stand | Pose::Sprint => 0,
            Pose::Crouch => 1,
            Pose::Prone => 2,
        }
    }

    /// Whether going from this pose to `other` passes the crouch (standing and prone).
    pub fn far_from(self, other: Pose) -> bool {
        (self.lowness() - other.lowness()).abs() == 2
    }
}

/// The move triggers that ask for each pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoseTriggers {
    pub crouch: usize,
    pub prone: usize,
    pub sprint: usize,
}

impl Default for PoseTriggers {
    /// Torque3D's: crouch 3, prone 4, sprint 5.
    fn default() -> PoseTriggers {
        PoseTriggers {
            crouch: 3,
            prone: 4,
            sprint: 5,
        }
    }
}

impl PoseTriggers {
    /// The pose a move's triggers ask for, before the room test (prone first, then crouch, then sprint).
    pub fn asked(&self, triggers: &[bool]) -> Pose {
        let held = |t: usize| triggers.get(t).copied().unwrap_or(false);
        if held(self.prone) {
            Pose::Prone
        } else if held(self.crouch) {
            Pose::Crouch
        } else if held(self.sprint) {
            Pose::Sprint
        } else {
            Pose::Stand
        }
    }
}

/// A pose's three speed caps: forward, backward and to the side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseSpeeds {
    pub forward: f32,
    pub backward: f32,
    pub side: f32,
}

/// The datablock's poses (Torque3D's `PlayerData` fields, named as there). Off by default: a TGE datablock allows none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseData {
    /// Which poses the datablock allows (a host sets these: Torque3D needs the shape's `crouch_root` and `prone_root`).
    pub crouch: bool,
    pub prone: bool,
    pub sprint: bool,
    /// `crouchBoundingBox` and `proneBoundingBox` (width, depth, height).
    pub crouch_box: F3,
    pub prone_box: F3,
    /// `maxCrouchForwardSpeed`, `maxCrouchBackwardSpeed`, `maxCrouchSideSpeed`, and the same for prone and sprint.
    pub crouch_speeds: PoseSpeeds,
    pub prone_speeds: PoseSpeeds,
    pub sprint_speeds: PoseSpeeds,
    /// `crouchForce`, `proneForce` and `sprintForce`: the pose's run force.
    pub crouch_force: f32,
    pub prone_force: f32,
    pub sprint_force: f32,
    /// `sprintCanJump`.
    pub sprint_can_jump: bool,
    pub triggers: PoseTriggers,
}

impl Default for PoseData {
    /// Torque3D's constructor values, with every pose off.
    fn default() -> PoseData {
        PoseData {
            crouch: false,
            prone: false,
            sprint: false,
            crouch_box: [1.0, 1.0, 1.0],
            prone_box: [1.0, 1.0, 1.0],
            crouch_speeds: PoseSpeeds {
                forward: 4.0,
                backward: 4.0,
                side: 4.0,
            },
            prone_speeds: PoseSpeeds {
                forward: 2.0,
                backward: 2.0,
                side: 0.0,
            },
            sprint_speeds: PoseSpeeds {
                forward: 15.0,
                backward: 10.0,
                side: 10.0,
            },
            crouch_force: 45.0 * 9.0,
            prone_force: 45.0 * 9.0,
            sprint_force: 50.0 * 9.0,
            sprint_can_jump: true,
            triggers: PoseTriggers::default(),
        }
    }
}

impl PoseData {
    /// Whether the datablock allows any pose besides standing.
    pub fn any(&self) -> bool {
        self.crouch || self.prone || self.sprint
    }

    /// Whether the datablock allows `pose`.
    pub fn allows(&self, pose: Pose) -> bool {
        match pose {
            Pose::Stand => true,
            Pose::Sprint => self.sprint,
            Pose::Crouch => self.crouch,
            Pose::Prone => self.prone,
        }
    }

    /// The pose's box size (`boundingBox` standing or sprinting).
    pub fn box_size(&self, db: &PlayerData, pose: Pose) -> F3 {
        match pose {
            Pose::Stand | Pose::Sprint => db.bounding_box,
            Pose::Crouch => self.crouch_box,
            Pose::Prone => self.prone_box,
        }
    }

    /// The pose's speeds (the datablock's own standing).
    pub fn speeds(&self, db: &PlayerData, pose: Pose) -> PoseSpeeds {
        match pose {
            Pose::Stand => PoseSpeeds {
                forward: db.max_forward_speed,
                backward: db.max_backward_speed,
                side: db.max_side_speed,
            },
            Pose::Sprint => self.sprint_speeds,
            Pose::Crouch => self.crouch_speeds,
            Pose::Prone => self.prone_speeds,
        }
    }

    /// The pose a record's move shows (its server's choice, [`PoseData::show`]): the allowed pose whose trigger is held, prone
    /// first, else standing.
    pub fn shown(&self, triggers: &[bool]) -> Pose {
        let held = |t: usize| triggers.get(t).copied().unwrap_or(false);
        let t = &self.triggers;
        if self.prone && held(t.prone) {
            Pose::Prone
        } else if self.crouch && held(t.crouch) {
            Pose::Crouch
        } else if self.sprint && held(t.sprint) {
            Pose::Sprint
        } else {
            Pose::Stand
        }
    }

    /// The move's triggers as a record shows `pose`: each allowed pose's trigger held only for that pose; the other triggers
    /// (a game's own use of the ones no pose takes) kept.
    pub fn show(&self, pose: Pose, triggers: &mut [bool]) {
        let t = &self.triggers;
        for (allowed, trigger, this) in [
            (self.crouch, t.crouch, Pose::Crouch),
            (self.prone, t.prone, Pose::Prone),
            (self.sprint, t.sprint, Pose::Sprint),
        ] {
            if let (true, Some(slot)) = (allowed, triggers.get_mut(trigger)) {
                *slot = pose == this;
            }
        }
    }

    /// The pose's run force.
    pub fn force(&self, db: &PlayerData, pose: Pose) -> f32 {
        match pose {
            Pose::Stand => db.run_force,
            Pose::Sprint => self.sprint_force,
            Pose::Crouch => self.crouch_force,
            Pose::Prone => self.prone_force,
        }
    }
}

/// The object box of a box size: half its width and depth each side, from 0 up its height (`Player::setPose`).
pub fn object_box(size: F3) -> Box3 {
    let (hx, hy) = (size[0] * 0.5, size[1] * 0.5);
    Box3::new([-hx, -hy, 0.0], [hx, hy, size[2]])
}

/// `setPose`: the pose and its object box (world box and scaled box with it); a change marks the move for the clients.
pub fn set_pose(s: &mut PlayerState, db: &PlayerData, pose: Pose) {
    if s.pose == pose {
        return;
    }
    s.pose_from = s.pose;
    s.pose_ticks = 0;
    s.pose = pose;
    s.obj_box = object_box(db.poses.box_size(db, pose));
    s.update_scaled_box();
    s.reset_world_box();
    s.set_mask(net_mask::MOVE);
}

/// Whether the Player may change pose at all now: in its move state, enabled and not mounted (crouching and lying down also
/// not falling).
fn free_to(s: &PlayerState, pose: Pose) -> bool {
    s.action_state == 1 && s.damage_state == 0 && s.mount.is_none() && !(pose.is_low() && s.falling)
}

/// Whether `pose` may be taken now: allowed, free to, and with room for its box when it is not the pose already
/// (standing and sprinting share a box).
pub fn can_take(
    p: &mut Player,
    db: &PlayerData,
    pose: Pose,
    host: &mut dyn PlayerHost,
) -> Result<bool, Exhausted> {
    let s = &p.s;
    if !db.poses.allows(pose) || !free_to(s, pose) {
        return Ok(false);
    }
    let same_box = s.pose == pose
        || (matches!(s.pose, Pose::Stand | Pose::Sprint)
            && matches!(pose, Pose::Stand | Pose::Sprint));
    if same_box {
        return Ok(true);
    }
    has_room(p, db.poses.box_size(db, pose), host)
}

/// Whether a box of `size` at the Player's feet is clear of the static world (see the module docs).
pub fn has_room(p: &mut Player, size: F3, host: &mut dyn PlayerHost) -> Result<bool, Exhausted> {
    let s = &p.s;
    let pos = s.position();
    let k = s.scale;
    let half = [size[0] * 0.5 * k[0], size[1] * 0.5 * k[1]];
    let min = [
        pos[0] - half[0] + ROOM_INSET,
        pos[1] - half[1] + ROOM_INSET,
        pos[2] + ROOM_FLOOR,
    ];
    let max = [
        pos[0] + half[0] - ROOM_INSET,
        pos[1] + half[1] - ROOM_INSET,
        pos[2] + size[2] * k[2],
    ];
    if !(min[2] < max[2]) {
        return Ok(true);
    }
    let room = Box3::new(min, max);
    let mask = host.type_masks().player_move_collide
        & !(types::PLAYER
            | types::VEHICLE
            | types::WATER
            | types::PHYSICAL_ZONE
            | types::TRIGGER
            | types::ITEM
            | types::CORPSE);
    let mut set = WorkingSet::new();
    set.update(host.world(), s.id, &room, mask)?;
    let mut list = ClippedPolyList::new();
    list.planes.extend_from_slice(&box_planes(min, max));
    for c in set.iter() {
        let world = host.world();
        let Some(bb) = c.bounding_box(world) else {
            continue;
        };
        if !room.overlaps(&bb) {
            continue;
        }
        c.poly_list(world, &mut list)?;
        if !list.polys.is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `updateMove`'s pose choice (see the module docs): the pose the move asks for when it may be taken, else standing when there
/// is room, else (from prone) crouched when there is room for that, else the pose kept. A mounted Player stands.
pub fn update_pose(
    p: &mut Player,
    db: &PlayerData,
    mv: &Move,
    host: &mut dyn PlayerHost,
) -> Result<(), Exhausted> {
    let d = &db.poses;
    if !d.any() {
        return Ok(());
    }
    if p.s.mount.is_some() {
        set_pose(&mut p.s, db, Pose::Stand);
        return Ok(());
    }
    let asked = d.triggers.asked(&mv.triggers);
    let mut tries = Vec::with_capacity(3);
    // Torque3D's chain: prone, crouch and sprint each when asked for, then standing.
    if asked == Pose::Prone {
        tries.push(Pose::Prone);
    }
    if mv.triggers.get(d.triggers.crouch).copied().unwrap_or(false) {
        tries.push(Pose::Crouch);
    }
    if mv.triggers.get(d.triggers.sprint).copied().unwrap_or(false) {
        tries.push(Pose::Sprint);
    }
    tries.push(Pose::Stand);
    if p.s.pose == Pose::Prone {
        tries.push(Pose::Crouch);
    }
    p.s.pose_ticks = p.s.pose_ticks.saturating_add(1);
    for pose in tries {
        if !can_take(p, db, pose, host)? {
            continue;
        }
        // Between standing and prone by way of the crouch, kneeling there for its ticks (when it may crouch).
        let s = &p.s;
        let kneeling =
            s.pose == Pose::Crouch && s.pose_from.far_from(pose) && s.pose_ticks < KNEEL_TICKS;
        if (s.pose.far_from(pose) || kneeling) && can_take(p, db, Pose::Crouch, host)? {
            set_pose(&mut p.s, db, Pose::Crouch);
            return Ok(());
        }
        set_pose(&mut p.s, db, pose);
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
#[path = "pose_tests.rs"]
mod tests;
