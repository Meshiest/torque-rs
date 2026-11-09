//! The joints in the props' world ([`PropWorld`]): adding and removing them, their caps, what each tick does to them (a
//! winch's reel, a grapple's pull and a hold's drive before the step; the distance, the force, the limit and the break after
//! it), and the tether a grapple puts on its player.

use std::collections::BTreeMap;

use torque_game::player_tether::Tether;
use torque_physics_rapier::{Anchor, BodyId, BodyState, JointDesc, JointId, JointKind, TICK};

use super::records::{MASK_FLAGS, MASK_INIT, MASK_LENGTH, MASK_SPRING};
use super::{
    AT_LIMIT, End, Hinge, Joint, JointError, JointEvent, Kind, KindId, Reach, Winch, number,
};
use crate::quant::{self, Quantized};
use crate::world::{Owner, OwnerKey, PlayerAt, PropWorld, TickReport, length, world_half};

/// The point `local` of a prop's frame at the state `q`, in the world: the same arithmetic on the server and on a client.
pub fn point_on(q: &Quantized, local: [f32; 3]) -> [f32; 3] {
    let m = quant::matrix(q.position(), q.rotation());
    std::array::from_fn(|r| {
        m[r * 4] * local[0] + m[r * 4 + 1] * local[1] + m[r * 4 + 2] * local[2] + m[r * 4 + 3]
    })
}

impl PropWorld {
    /// The joints by id.
    pub fn joints(&self) -> &BTreeMap<u32, Joint> {
        &self.joints
    }

    pub fn joint(&self, id: u32) -> Option<&Joint> {
        self.joints.get(&id)
    }

    /// The joints `key` owns.
    pub fn joints_owned_by(&self, key: &OwnerKey) -> u32 {
        self.joints
            .values()
            .filter(|j| j.owner.as_ref().is_some_and(|o| &o.key == key))
            .count() as u32
    }

    /// The joints the owners of `address` have, orphans counted.
    pub fn joints_owned_from(&self, address: &str) -> u32 {
        self.joints
            .values()
            .filter(|j| {
                j.owner
                    .as_ref()
                    .and_then(|o| o.address.as_deref())
                    .is_some_and(|a| a == address)
            })
            .count() as u32
    }

    /// Changes who owns a joint (an owner who left, or came back).
    pub fn set_joint_owner(&mut self, id: u32, owner: Option<Owner>) -> bool {
        self.joints.get_mut(&id).map(|j| j.owner = owner).is_some()
    }

    /// Takes out a joint whose owner has been gone longer than the orphan time; its object (and rope) is for the caller to
    /// delete ([`PropWorld::take_gone_joints`]).
    pub fn expire_joint(&mut self, id: u32) {
        if let Some(j) = self.remove_joint(id) {
            self.gone_joints.push((id, j.rope));
        }
    }

    /// The joints prop `id` is an end of.
    pub fn joints_on(&self, id: u32) -> u32 {
        self.joints
            .values()
            .filter(|j| j.props().any(|p| p == id))
            .count() as u32
    }

    /// Whether a joint of `kind` between `a` and `b` owned by `owner` may be made: the ends (props there; a player as a
    /// grapple's first end and only there, with no grapple on it yet; the world as a weld's second end and only there; points and
    /// offsets inside the world; not one thing twice, not two points) and the caps (the owner's, its address's, each prop's,
    /// the server's).
    pub fn may_join(
        &self,
        kind: KindId,
        a: &End,
        b: &End,
        owner: Option<&Owner>,
    ) -> Result<(), JointError> {
        // Inside the world: what the wire carries unchanged (a client holds what it decodes to the same bound).
        let inside = |v: &[f32; 3]| {
            v.iter()
                .all(|c| c.is_finite() && c.abs() <= quant::MAX_COORD)
        };
        let end_ok = |e: &End, first: bool| match *e {
            End::Prop { id, local } => self.props().contains_key(&id) && inside(&local),
            // A weld holds to the world as a whole (its frame is the world's), never to a point of it.
            End::Point(p) => {
                kind != KindId::Weld
                    && p.iter()
                        .all(|v| v.is_finite() && v.abs() < quant::MAX_COORD)
            }
            End::Player { offset, .. } => first && kind.from_player() && inside(&offset),
            End::World => !first && kind == KindId::Weld,
        };
        let first_ok = match kind {
            KindId::Grapple => matches!(a, End::Player { .. }),
            KindId::Hold => matches!((a, b), (End::Player { .. }, End::Prop { .. })),
            KindId::Weld | KindId::Hinge => matches!(a, End::Prop { .. }),
            KindId::Tether | KindId::Winch => true,
        };
        let same = match (a, b) {
            (End::Prop { id: x, .. }, End::Prop { id: y, .. }) => x == y,
            (End::Point(_), End::Point(_)) => true,
            _ => false,
        };
        if !first_ok || !end_ok(a, true) || !end_ok(b, false) || same {
            return Err(JointError::Ends);
        }
        if let End::Player { id, .. } = *a
            && kind == KindId::Grapple
            && self.grapple_on(id).is_some()
        {
            return Err(JointError::Grappled);
        }
        if kind == KindId::Hold
            && let (End::Player { id, .. }, Some(prop)) = (a, b.prop())
        {
            if self.hold_on(*id).is_some() || self.holder_of(prop).is_some() {
                return Err(JointError::Held);
            }
            if self.get(prop).is_some_and(|p| p.rest.is_frozen()) {
                return Err(JointError::Frozen);
            }
        }
        if self.joints.len() as u32 >= self.joint_limits.per_server {
            return Err(JointError::PerServer);
        }
        if let Some(o) = owner {
            if self.joints_owned_by(&o.key) >= self.joint_limits.per_player {
                return Err(JointError::PerPlayer);
            }
            if let Some(a) = o.address.as_deref()
                && self.joints_owned_from(a) >= self.joint_limits.per_address
            {
                return Err(JointError::PerAddress);
            }
        }
        if [a, b]
            .iter()
            .filter_map(|e| e.prop())
            .any(|p| self.joints_on(p) >= self.joint_limits.per_prop)
        {
            return Err(JointError::PerProp);
        }
        Ok(())
    }

    /// Adds joint `id` (its caps and ends checked, [`PropWorld::may_join`]); the mask bits to send.
    pub fn add_joint(&mut self, id: u32, joint: Joint) -> Result<u32, JointError> {
        if self.joints.contains_key(&id) || self.props().contains_key(&id) {
            return Err(JointError::Taken);
        }
        self.may_join(joint.kind.id(), &joint.a, &joint.b, joint.owner.as_ref())?;
        let mut joint = joint;
        joint.break_force = number(joint.break_force, 0.0, super::MAX_FORCE);
        joint.status = super::Status::default();
        if let Some(desc) = self.rapier_desc(&joint) {
            match self.physics_mut().add_joint(JointId(id), &desc) {
                Ok(()) => {}
                Err(torque_physics_rapier::Error::NonFinite) => return Err(JointError::Bad),
                Err(_) => return Err(JointError::World),
            }
        }
        let props: Vec<u32> = joint.props().collect();
        let hold = joint.kind == Kind::Hold;
        self.joints.insert(id, joint);
        for p in props {
            self.rouse(p);
            if hold {
                self.held_changed(p);
            }
        }
        Ok(MASK_INIT | MASK_LENGTH | MASK_SPRING | MASK_FLAGS)
    }

    /// Takes a joint away (out of the physics world too); the joint, when it was one.
    pub fn remove_joint(&mut self, id: u32) -> Option<Joint> {
        let joint = self.joints.remove(&id)?;
        let removed = self.physics_mut().remove_joint(JointId(id));
        self.note(&format!("joint {id}"), removed);
        if joint.kind == Kind::Hold
            && !joint.status.broken
            && let Some(p) = joint.b.prop()
        {
            self.held_changed(p);
        }
        Some(joint)
    }

    /// The joints taken out since the last call because a body at an end went, with the ropes that showed them, for the caller
    /// to delete their objects.
    pub fn take_gone_joints(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.gone_joints)
    }

    /// Where the players at grapples' and holds' ends are now (a joint whose player is not there is taken out at the next
    /// tick).
    pub fn set_players(&mut self, players: BTreeMap<u32, PlayerAt>) {
        self.players = players;
    }

    /// The rope that shows joint `id`.
    pub fn set_rope(&mut self, id: u32, rope: u32) -> bool {
        self.joints.get_mut(&id).map(|j| j.rope = rope).is_some()
    }

    /// Changes a tether's, a winch's or a grapple's reach (held to its ranges); the mask bits to send. None for a joint that
    /// has no reach.
    pub fn set_reach(&mut self, id: u32, f: impl FnOnce(&mut Reach)) -> Option<u32> {
        let joint = self.joints.get_mut(&id)?;
        let reach = joint.kind.reach_mut()?;
        f(reach);
        *reach = reach.held();
        self.reapply(id);
        Some(MASK_LENGTH | MASK_SPRING)
    }

    /// Makes a tether a winch reeling to `winch.target` (held to its ranges); the mask bits to send. None for a joint that is
    /// not a tether.
    pub fn set_winch(&mut self, id: u32, winch: Winch) -> Option<u32> {
        let joint = self.joints.get_mut(&id)?;
        let Kind::Tether { winch: w, .. } = &mut joint.kind else {
            return None;
        };
        *w = Some(winch.held());
        Some(MASK_INIT)
    }

    /// The force past which joint `id` breaks (0: never).
    pub fn set_break_force(&mut self, id: u32, force: f32) -> bool {
        self.joints
            .get_mut(&id)
            .map(|j| j.break_force = number(force, 0.0, super::MAX_FORCE))
            .is_some()
    }

    /// Changes a hinge's limits or motor (held to their ranges). None for a joint that is not a hinge.
    pub fn set_hinge(&mut self, id: u32, f: impl FnOnce(&mut Hinge)) -> Option<u32> {
        let joint = self.joints.get_mut(&id)?;
        let Kind::Hinge(h) = &mut joint.kind else {
            return None;
        };
        f(h);
        let pi = std::f32::consts::PI;
        h.limits = h.limits.map(|[lo, hi]| {
            let lo = number(lo, -pi, pi);
            [lo, number(hi, lo, pi)]
        });
        h.motor = h.motor.map(|m| super::Motor {
            speed: number(m.speed, -super::MAX_MOTOR_SPEED, super::MAX_MOTOR_SPEED),
            max_torque: number(m.max_torque, 0.0, super::MAX_FORCE),
        });
        self.reapply(id);
        Some(0)
    }

    /// A joint's kind given to the physics world again after a change.
    fn reapply(&mut self, id: u32) {
        let Some(joint) = self.joints.get(&id) else {
            return;
        };
        let props: Vec<u32> = joint.props().collect();
        if let Some(desc) = self.rapier_desc(joint) {
            let set = self.physics_mut().set_joint(JointId(id), desc.kind);
            self.note(&format!("joint {id}"), set);
        }
        for p in props {
            self.rouse(p);
        }
    }

    /// Where an end is in the world now: a prop's point at its state (at the wire's precision), a point, a player's held point
    /// (none for the world, which is everywhere).
    pub fn end_point(&self, end: &End) -> Option<[f32; 3]> {
        match *end {
            End::Prop { id, local } => Some(point_on(&self.get(id)?.q, local)),
            End::Point(p) => Some(p),
            End::Player { id, offset } => {
                let p = self.players.get(&id)?.position;
                Some(std::array::from_fn(|k| p[k] + offset[k]))
            }
            End::World => None,
        }
    }

    /// The point of the world `point` in prop `id`'s frame.
    pub fn local_of(&self, id: u32, point: [f32; 3]) -> Option<[f32; 3]> {
        self.physics().to_local(BodyId(id), point)
    }

    /// A direction of the world in prop `id`'s frame.
    pub fn local_dir_of(&self, id: u32, dir: [f32; 3]) -> Option<[f32; 3]> {
        self.physics().to_local_dir(BodyId(id), dir)
    }

    /// Where `b` is in `a`'s frame now (a prop, or the world for none): a weld's frame.
    pub fn weld_frame(&self, a: Option<u32>, b: Option<u32>) -> Option<([f32; 3], [f32; 4])> {
        self.physics().relative_pose(a.map(BodyId), b.map(BodyId))
    }

    /// The grapple on `player`, broken or not: a player has at most one ([`PropWorld::may_join`]), so the server's tether,
    /// its pull and a client's prediction all find the same one.
    pub fn grapple_on(&self, player: u32) -> Option<u32> {
        self.joints.iter().find_map(|(&id, j)| {
            (matches!(j.kind, Kind::Grapple { .. })
                && matches!(j.a, End::Player { id, .. } if id == player))
            .then_some(id)
        })
    }

    /// The unbroken hold `player` carries a prop by, if it carries one.
    pub fn hold_on(&self, player: u32) -> Option<u32> {
        self.joints.iter().find_map(|(&id, j)| {
            (j.kind == Kind::Hold
                && !j.status.broken
                && matches!(j.a, End::Player { id, .. } if id == player))
            .then_some(id)
        })
    }

    /// The player holding prop `prop` by an unbroken hold, if one is.
    pub fn holder_of(&self, prop: u32) -> Option<u32> {
        self.holds()
            .find_map(|(_, player, p)| (p == prop).then_some(player))
    }

    /// Each held prop's holder, by prop.
    pub fn holders(&self) -> BTreeMap<u32, u32> {
        self.holds().map(|(_, player, p)| (p, player)).collect()
    }

    /// The props held now.
    pub fn held_props(&self) -> impl Iterator<Item = u32> + '_ {
        self.holds().map(|(_, _, p)| p)
    }

    /// The unbroken holds: each one's id, its player and its prop.
    fn holds(&self) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
        self.joints
            .iter()
            .filter(|(_, j)| !j.status.broken)
            .filter_map(|(&id, j)| match (j.kind, j.a, j.b) {
                (Kind::Hold, End::Player { id: player, .. }, End::Prop { id: p, .. }) => {
                    Some((id, player, p))
                }
                _ => None,
            })
    }

    /// Breaks hold `id` (it holds nothing from then on): its prop is solid to its holder again (and pushed out of any player's
    /// box by that player's kinematic box). False for a joint that is not an unbroken hold.
    pub(crate) fn break_hold(&mut self, id: u32) -> bool {
        let Some(j) = self
            .joints
            .get_mut(&id)
            .filter(|j| j.kind == Kind::Hold && !j.status.broken)
        else {
            return false;
        };
        j.status.broken = true;
        if let Some(p) = j.b.prop() {
            self.held_changed(p);
        }
        true
    }

    /// The holds on prop `prop` broken (it was frozen), their breaks reported at the next tick.
    pub(crate) fn break_holds_on(&mut self, prop: u32) {
        let holds: Vec<u32> = self
            .holds()
            .filter(|&(_, _, p)| p == prop)
            .map(|(id, ..)| id)
            .collect();
        for id in holds {
            if self.break_hold(id) {
                self.broken_holds.push(id);
            }
        }
    }

    /// Where the hold on a player at `at` carries its prop: the point `offset` of the player's frame, upright, facing where
    /// the player faces.
    pub fn hold_pose(at: &PlayerAt, offset: [f32; 3]) -> ([f32; 3], [f32; 4]) {
        (at.point(offset), at.turn())
    }

    /// The tether the grapple on `player` puts on it: none when it has none, it broke, or its far end is nowhere.
    pub fn tether_of(&self, player: u32) -> Option<Tether> {
        let j = self.joints.get(&self.grapple_on(player)?)?;
        let (Kind::Grapple { reach }, End::Player { offset, .. }) = (j.kind, j.a) else {
            return None;
        };
        if j.status.broken {
            return None;
        }
        let anchor = self.end_point(&j.b)?;
        let spring = reach.spring.unwrap_or(super::Spring {
            stiffness: 0.0,
            damping: 0.0,
        });
        Some(Tether {
            anchor,
            offset,
            length: reach.max,
            stiffness: spring.stiffness,
            damping: spring.damping,
        })
    }

    /// The grapple on `player` changed its velocity by `change` (the player `mass` heavy): the prop at its far end takes the
    /// opposite impulse at the next step, by the pair's reduced mass ([`super::grapple_pull`]; a frozen or missing prop is a
    /// fixed end, and no prop moves), and the Player's momentum counts toward its break force ([`super::grapple_momentum`]).
    pub fn pulled(&mut self, player: u32, change: [f32; 3], mass: f32) {
        let Some(id) = self.grapple_on(player) else {
            return;
        };
        let far = self
            .joints
            .get(&id)
            .and_then(|j| j.b.prop())
            .and_then(|p| self.get(p))
            .map_or(super::FarEnd::Fixed, super::FarEnd::of);
        let Some(joint) = self.joints.get_mut(&id).filter(|j| !j.status.broken) else {
            return;
        };
        let (Some(impulse), Some(momentum)) = (
            super::grapple_pull(change, mass, far),
            super::grapple_momentum(change, mass),
        ) else {
            return;
        };
        for (p, i) in joint.pending.iter_mut().zip(impulse) {
            *p += i;
        }
        joint.pull += length(momentum) / TICK;
    }

    /// The physics world's joint for a joint (none for a grapple, which is not a physics joint, or a broken one).
    fn rapier_desc(&self, joint: &Joint) -> Option<JointDesc> {
        if joint.status.broken {
            return None;
        }
        let anchor = |e: &End| match *e {
            End::Prop { id, local } => Some(Anchor::Body {
                body: BodyId(id),
                local,
            }),
            End::Point(p) => Some(Anchor::World(p)),
            End::World => Some(Anchor::World([0.0; 3])),
            End::Player { .. } => None,
        };
        let kind = match joint.kind {
            Kind::Grapple { .. } | Kind::Hold => return None,
            Kind::Tether { reach, .. } => JointKind::Distance {
                min: reach.min,
                max: reach.max,
                spring: reach.spring,
            },
            Kind::Weld { frame } => JointKind::Fixed { frame },
            Kind::Hinge(h) => JointKind::Revolute {
                axis_a: h.axis_a,
                axis_b: h.axis_b,
                limits: h.limits,
                motor: h.motor,
            },
        };
        Some(JointDesc {
            a: anchor(&joint.a)?,
            b: anchor(&joint.b)?,
            kind,
        })
    }

    /// The joints prop `id` was an end of, taken out with it.
    pub(crate) fn drop_joints_of(&mut self, id: u32) {
        let gone: Vec<u32> = self
            .joints
            .iter()
            .filter(|(_, j)| j.props().any(|p| p == id))
            .map(|(&j, _)| j)
            .collect();
        for j in gone {
            if let Some(joint) = self.remove_joint(j) {
                self.gone_joints.push((j, joint.rope));
            }
        }
    }

    /// The joints of prop `id` given to the physics world again (its body was made again), in id order.
    pub(crate) fn reinsert_joints_of(&mut self, id: u32) {
        let ids: Vec<u32> = self
            .joints
            .iter()
            .filter(|(_, j)| j.props().any(|p| p == id))
            .map(|(&j, _)| j)
            .collect();
        for j in ids {
            self.reinsert(j);
        }
    }

    /// Every joint given to a physics world made again, in id order; one it does not take breaks.
    pub(crate) fn reinsert_all_joints(&mut self) {
        let ids: Vec<u32> = self.joints.keys().copied().collect();
        for j in ids {
            self.reinsert(j);
        }
    }

    fn reinsert(&mut self, id: u32) {
        let Some(desc) = self.joints.get(&id).and_then(|j| self.rapier_desc(j)) else {
            return;
        };
        let removed = self.physics_mut().remove_joint(JointId(id));
        self.note(&format!("joint {id}"), removed);
        let added = self.physics_mut().add_joint(JointId(id), &desc);
        if added.is_err()
            && let Some(j) = self.joints.get_mut(&id)
        {
            // A joint the world does not take holds nothing from then on, as a broken one.
            j.status.broken = true;
        }
        self.note(&format!("joint {id} given again"), added);
    }

    /// Before the step: the holds broken since the last tick reported, grapples and holds whose players went taken out, each
    /// grapple's pull given to the prop at its far end, each winch reeled and each hold's prop driven.
    pub(crate) fn joints_before_step(&mut self, report: &mut TickReport) {
        for id in std::mem::take(&mut self.broken_holds) {
            report.joint_events.push((id, JointEvent::Break));
            report.joints.push((id, MASK_FLAGS));
        }
        let lost: Vec<u32> = self
            .joints
            .iter()
            .filter(
                |(_, j)| matches!(j.a, End::Player { id, .. } if !self.players.contains_key(&id)),
            )
            .map(|(&id, _)| id)
            .collect();
        for id in lost {
            if let Some(j) = self.remove_joint(id) {
                self.gone_joints.push((id, j.rope));
            }
        }
        let mut pulls = Vec::new();
        let mut reeled = Vec::new();
        for (&id, j) in self.joints.iter_mut() {
            let pending = std::mem::take(&mut j.pending);
            if let Some(p) = j.b.prop()
                && pending != [0.0; 3]
            {
                pulls.push((p, pending));
            }
            let force = j.status.force;
            let Kind::Tether {
                reach,
                winch: Some(w),
            } = &mut j.kind
            else {
                continue;
            };
            let now = reach.max;
            let step = w.rate * TICK;
            let next = if w.target < now {
                if w.max_force > 0.0 && force > w.max_force {
                    now
                } else {
                    (now - step).max(w.target)
                }
            } else {
                (now + step).min(w.target)
            };
            if next != now {
                reach.max = next;
                reach.min = reach.min.min(next);
                reeled.push(id);
            }
        }
        for (p, impulse) in pulls {
            // A frozen prop is pinned: it takes no pull.
            if self.get(p).is_some_and(|q| !q.rest.is_frozen()) {
                let pulled = self.physics_mut().apply_impulse(BodyId(p), impulse);
                self.note(&format!("a grapple's pull on prop {p}"), pulled);
            }
        }
        for id in reeled {
            self.reapply(id);
            report.joints.push((id, MASK_LENGTH));
        }
        self.drive_holds();
    }

    /// Each held prop given the speed and the spin that take it to its hold's pose by the end of the step (held to
    /// [`super::HOLD_SPEED`] and [`super::HOLD_SPIN`]), the step's fall made up, and woken; a pose that would put it in another
    /// player's box keeps it where it is.
    fn drive_holds(&mut self) {
        let config = self.physics().config();
        let substeps = f32::from(config.substeps.max(1));
        // A body set going at `v` falls `g T (n + 1) / 2n` short of `v T` over a tick of n semi-implicit sub-steps.
        let fall = config
            .gravity
            .map(|g| -g * TICK * (substeps + 1.0) / (2.0 * substeps));
        let holds: Vec<(u32, u32, [f32; 3])> = self
            .joints
            .values()
            .filter(|j| !j.status.broken)
            .filter_map(|j| match (j.kind, j.a, j.b) {
                (Kind::Hold, End::Player { id, offset }, End::Prop { id: p, .. }) => {
                    Some((p, id, offset))
                }
                _ => None,
            })
            .collect();
        for (prop, player, offset) in holds {
            let (Some(at), Some(now)) = (
                self.players.get(&player).copied(),
                self.physics().body(BodyId(prop)),
            ) else {
                continue;
            };
            if self.get(prop).is_none_or(|p| p.rest.is_frozen()) {
                continue;
            }
            self.rouse(prop);
            let (mut target, turn) = PropWorld::hold_pose(&at, offset);
            if let Some(half) = self
                .get(prop)
                .map(|p| world_half(p.params.shape.half_extents(), turn))
                && self.bodies.iter().any(|&(id, min, max)| {
                    id != player
                        && (0..3)
                            .all(|k| target[k] + half[k] > min[k] && target[k] - half[k] < max[k])
                })
            {
                target = now.position;
            }
            let linvel = clamp(
                std::array::from_fn(|k| (target[k] - now.position[k]) / TICK),
                super::HOLD_SPEED,
            );
            // The turn left to make, the short way: twice its vector part is its angle for a small turn, and turns a large one
            // no faster than the clamp.
            let left = quant::quat_mul(turn, quant::conjugate(now.rotation));
            let sign = if left[3] < 0.0 { -1.0 } else { 1.0 };
            let angvel = clamp(
                std::array::from_fn(|k| 2.0 * sign * left[k] / TICK),
                super::HOLD_SPIN,
            );
            let s = BodyState {
                linvel: std::array::from_fn(|k| linvel[k] + fall[k]),
                angvel,
                sleeping: false,
                ..now
            };
            let set = self.physics_mut().set_body(BodyId(prop), &s);
            self.note(&format!("the hold on prop {prop}"), set);
        }
    }

    /// After the step: each joint's distance and force read, a joint past its break force broken, the limit flags set.
    pub(crate) fn joints_after_step(&mut self, report: &mut TickReport) {
        let ids: Vec<u32> = self.joints.keys().copied().collect();
        for id in ids {
            let Some(j) = self.joints.get(&id) else {
                continue;
            };
            let (distance, force, torque) = match j.kind {
                Kind::Hold if j.status.broken => (j.status.distance, 0.0, 0.0),
                Kind::Hold => {
                    let d = match (j.a, j.b.prop().and_then(|p| self.get(p))) {
                        (End::Player { id, offset }, Some(p)) => match self.players.get(&id) {
                            Some(at) => {
                                let (target, _) = PropWorld::hold_pose(at, offset);
                                let now = p.q.position();
                                length(std::array::from_fn(|k| now[k] - target[k]))
                            }
                            None => j.status.distance,
                        },
                        _ => j.status.distance,
                    };
                    (d, 0.0, 0.0)
                }
                Kind::Grapple { .. } => {
                    let d = match (self.end_point(&j.a), self.end_point(&j.b)) {
                        (Some(a), Some(b)) => length(std::array::from_fn(|k| b[k] - a[k])),
                        _ => j.status.distance,
                    };
                    (d, j.pull, 0.0)
                }
                _ if j.status.broken => (j.status.distance, 0.0, 0.0),
                _ => match self.physics().joint(JointId(id)) {
                    Some(s) => (s.distance, s.force, s.torque),
                    None => (j.status.distance, 0.0, 0.0),
                },
            };
            let at_limit =
                !j.status.broken && j.kind.reach().is_some_and(|r| distance >= r.max - AT_LIMIT);
            let stuck = j.kind == Kind::Hold && !j.status.broken && distance > super::HOLD_BREAK;
            let breaks = stuck
                || !j.status.broken
                    && j.break_force > 0.0
                    && (force > j.break_force || torque > j.break_force);
            let before = (j.status.at_limit, j.status.broken);
            if stuck {
                self.break_hold(id);
                report.joint_events.push((id, JointEvent::Break));
            } else if breaks {
                let removed = self.physics_mut().remove_joint(JointId(id));
                self.note(&format!("joint {id}"), removed);
                report.joint_events.push((id, JointEvent::Break));
            }
            let Some(j) = self.joints.get_mut(&id) else {
                continue;
            };
            j.pull = 0.0;
            j.status.distance = if distance.is_finite() { distance } else { 0.0 };
            j.status.force = if force.is_finite() { force } else { 0.0 };
            j.status.torque = if torque.is_finite() { torque } else { 0.0 };
            j.status.broken |= breaks;
            j.status.at_limit = at_limit && !j.status.broken;
            if (j.status.at_limit, j.status.broken) != before {
                report.joints.push((id, MASK_FLAGS));
            }
        }
        report.joints.sort_by_key(|&(id, _)| id);
        report.joints.dedup_by(|b, a| {
            if a.0 == b.0 {
                a.1 |= b.1;
                true
            } else {
                false
            }
        });
    }

    /// The joints as words, in id order (the id, the kind, the length, the shortest, the distance and the force by their bits,
    /// the flags), for a trace hash.
    pub fn joint_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.joints.len() * 7);
        for (&id, j) in &self.joints {
            let reach = j.kind.reach().copied().unwrap_or(Reach {
                min: 0.0,
                max: 0.0,
                spring: None,
            });
            out.extend([
                id,
                j.kind.id().number(),
                reach.max.to_bits(),
                reach.min.to_bits(),
                j.status.distance.to_bits(),
                j.status.force.to_bits(),
                u32::from(j.status.at_limit) | u32::from(j.status.broken) << 1,
            ]);
        }
        out
    }
}

/// `v` no longer than `max`.
fn clamp(v: [f32; 3], max: f32) -> [f32; 3] {
    let len = length(v);
    if len > max && len.is_finite() {
        v.map(|c| c * (max / len))
    } else if len.is_finite() {
        v
    } else {
        [0.0; 3]
    }
}
