//! Props that are children in the scene hierarchy, as the physics world holds them.
//!
//! A prop hung on a dynamic prop is a **part**: its collider sits on the root prop's body at a fixed pose in the root's frame
//! ([`Link::Part`]), so the pair is one rigid body with both masses, and nothing drifts between them. A prop hung on anything
//! else (a static or kinematic object, a Player, a frozen prop, an object that is no prop) is **driven**: its body is
//! kinematic and is taken to the pose the hierarchy gives it each tick, pushing the props it meets and pushed by none
//! ([`Link::Driven`]). Unparented, a prop is a dynamic body again, leaving at the speed its place gave it. The glue that reads the
//! hierarchy is [`crate::hierarchy`]; nothing here knows a script engine.
//!
//! The children are indexed ([`Kids`]): the driven ones, and the parts of each root with the state their last settle saw, so a tick
//! costs what moved, not what exists. A prop's link changes only through [`PropWorld::set_link`], which keeps the index whole.

use std::collections::{BTreeMap, BTreeSet};

use torque_collide::math::matrix_product;
use torque_physics_rapier::{BodyDesc, BodyId, BodyState, Motion, PartDesc};

use super::{PropWorld, TickReport, body_shape};
use crate::quant::{self, Quantized};
use crate::records::{MASK_FLAGS, MASK_STATE};

/// A position and a unit quaternion `x, y, z, w` of the physics world.
pub type Pose = ([f32; 3], [f32; 4]);

/// How the scene hierarchy holds a prop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Link {
    /// A body of its own, moved by the world: not a child.
    #[default]
    Free,
    /// A child of anything but a dynamic prop: a kinematic body taken each tick to the pose the hierarchy gives it.
    Driven {
        /// The speed and the spin of the last move (what it leaves with when it is unparented).
        linvel: [f32; 3],
        angvel: [f32; 3],
        /// Where the last move took it: a tick whose target is the same, with the body at rest, does nothing.
        at: Option<Pose>,
        /// It was frozen when it was hung (it is frozen again when it is let go, if its datablock still lets it).
        was_frozen: bool,
    },
    /// A child of a dynamic prop `root` (or of a part of it): a collider on `root`'s body at `local`, a pose in its frame.
    Part { root: u32, local: Pose },
}

impl Link {
    /// Whether this is a child.
    #[must_use]
    pub fn is_child(&self) -> bool {
        *self != Link::Free
    }

    /// A driven link with nothing moved yet.
    fn driven(was_frozen: bool) -> Link {
        Link::Driven {
            linvel: [0.0; 3],
            angvel: [0.0; 3],
            at: None,
            was_frozen,
        }
    }
}

/// The parts of one root: their ids, and the root's state the last settle placed them from.
#[derive(Debug, Default)]
struct PartGroup {
    ids: BTreeSet<u32>,
    seen: Option<Quantized>,
}

/// The children of a world by kind, kept by [`PropWorld::set_link`].
#[derive(Debug, Default)]
pub(super) struct Kids {
    driven: BTreeSet<u32>,
    parts: BTreeMap<u32, PartGroup>,
}

/// `local` in the frame of a body at `root`'s state: where a part is in the world.
pub(crate) fn part_pose(root: &Quantized, local: Pose) -> Option<Pose> {
    let at = matrix_product(
        &quant::matrix(root.position(), root.rotation()),
        &quant::matrix(local.0, local.1),
    );
    quant::pose_of(&at)
}

impl PropWorld {
    /// How the hierarchy holds prop `id` (none for no such prop).
    #[must_use]
    pub fn link(&self, id: u32) -> Option<Link> {
        self.props.get(&id).map(|p| p.link)
    }

    /// Whether `id` is a prop and a child.
    #[must_use]
    pub fn is_child(&self, id: u32) -> bool {
        self.link(id).is_some_and(|l| l.is_child())
    }

    /// Whether `id` is a prop with a body of its own that moves (not a child, not frozen): what a prop hung on it joins.
    #[must_use]
    pub fn is_dynamic(&self, id: u32) -> bool {
        self.props
            .get(&id)
            .is_some_and(|p| p.link == Link::Free && !p.rest.is_frozen())
    }

    /// The props that are parts of `root`'s body, in id order.
    #[must_use]
    pub fn parts_of(&self, root: u32) -> Vec<u32> {
        self.kids
            .parts
            .get(&root)
            .map_or_else(Vec::new, |g| g.ids.iter().copied().collect())
    }

    /// The driven props, in id order.
    pub fn driven(&self) -> impl Iterator<Item = u32> + '_ {
        self.kids.driven.iter().copied()
    }

    /// Every child, in id order within each kind (the driven ones, then the parts by root).
    pub fn children(&self) -> impl Iterator<Item = u32> + '_ {
        self.driven()
            .chain(self.kids.parts.values().flat_map(|g| g.ids.iter().copied()))
    }

    /// The roots whose bodies changed in a way their children must follow (frozen, thawed or removed) since the last call.
    pub(crate) fn take_dirty_roots(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.dirty_roots)
    }

    /// Prop `id`'s link, with the index of the children kept whole. Does nothing for a prop that is not there.
    pub(super) fn set_link(&mut self, id: u32, new: Link) {
        let Some(p) = self.props.get_mut(&id) else {
            return;
        };
        let old = std::mem::replace(&mut p.link, new);
        self.unindex(id, old);
        match new {
            Link::Free => {}
            Link::Driven { .. } => {
                self.kids.driven.insert(id);
            }
            Link::Part { root, .. } => {
                let group = self.kids.parts.entry(root).or_default();
                group.ids.insert(id);
                group.seen = None;
            }
        }
    }

    /// Takes `id` out of the index of the kind `link` was.
    pub(super) fn unindex(&mut self, id: u32, link: Link) {
        match link {
            Link::Free => {}
            Link::Driven { .. } => {
                self.kids.driven.remove(&id);
            }
            Link::Part { root, .. } => {
                if let Some(group) = self.kids.parts.get_mut(&root) {
                    group.ids.remove(&id);
                    if group.ids.is_empty() {
                        self.kids.parts.remove(&root);
                    }
                }
            }
        }
    }

    /// Whether the step's budget for bodies made for the hierarchy this tick is spent: a fit refused for it is tried again next tick.
    pub(crate) fn build_exhausted(&self) -> bool {
        self.build_spent >= self.limits.step_budget
    }

    /// The body of prop `id` as `motion` at `s` (its shape and surface from its datablock). A `budgeted` build is charged to the
    /// tick's budget for bodies made and refused once it is spent; putting a prop back as it was is never refused.
    fn put_body(&mut self, id: u32, motion: Motion, s: &BodyState, budgeted: bool) -> bool {
        let Some(p) = self.props.get(&id) else {
            return false;
        };
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
        let (mut spent, budget) = (self.build_spent, self.limits.step_budget);
        let mut charge = |units: u64| {
            spent = spent.saturating_add(units);
            !budgeted || spent <= budget
        };
        let added = self.world.add_body(BodyId(id), &desc, &mut charge);
        self.build_spent = spent;
        let ok = added.is_ok();
        self.note(&format!("prop {id}'s body"), added);
        ok
    }

    /// The part collider of prop `id` on `root`'s body (charged as [`PropWorld::put_body`] says).
    pub(super) fn put_part(&mut self, id: u32, root: u32, local: Pose, budgeted: bool) -> bool {
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        let mut desc = PartDesc::new(body_shape(&p.params, p.hulls.as_deref()), local.0, local.1);
        desc.density = p.params.density;
        desc.friction = p.params.friction;
        desc.restitution = p.params.restitution;
        let (mut spent, budget) = (self.build_spent, self.limits.step_budget);
        let mut charge = |units: u64| {
            spent = spent.saturating_add(units);
            !budgeted || spent <= budget
        };
        let added = self
            .world
            .add_part(BodyId(id), BodyId(root), &desc, &mut charge);
        self.build_spent = spent;
        let ok = added.is_ok();
        self.note(&format!("prop {id}'s part"), added);
        ok
    }

    /// Takes prop `id` out of the physics world in whatever form it is in, giving where it was (with the speed its place gave it).
    fn leave(&mut self, id: u32) -> Option<BodyState> {
        let link = self.props.get(&id)?.link;
        let state = match link {
            Link::Part { .. } => match self.world.detach_part(BodyId(id)) {
                Ok(state) => state,
                Err(e) => {
                    self.note(&format!("prop {id}'s part"), Err::<(), _>(e));
                    return None;
                }
            },
            Link::Free | Link::Driven { .. } => {
                let mut s = self.world.body(BodyId(id))?;
                if let Link::Driven { linvel, angvel, .. } = link {
                    (s.linvel, s.angvel) = (linvel, angvel);
                }
                let removed = self.world.remove_body(BodyId(id));
                self.note(&format!("prop {id}'s body"), removed);
                s
            }
        };
        self.revision = super::next_revision();
        Some(state)
    }

    /// Puts prop `id` back in the form it had before a change that failed, at `s`. A prop a working world will not hold again is
    /// taken out and reported ([`TickReport::dropped`]), so its object is deleted rather than left without a body; false then. In a
    /// stopped world nothing can be put back: the next tick makes every prop again from its state and its link, so the prop stays.
    /// True when it was put back.
    fn restore_form(&mut self, id: u32, was: Link, s: &BodyState) -> bool {
        let frozen = self.props.get(&id).is_some_and(|p| p.rest.is_frozen());
        let made = match was {
            Link::Free => {
                let motion = if frozen {
                    Motion::Fixed
                } else {
                    Motion::Dynamic
                };
                self.put_body(id, motion, s, false)
            }
            Link::Driven { .. } => self.put_body(id, Motion::Kinematic, s, false),
            Link::Part { root, local } => self.put_part(id, root, local, false),
        };
        if !made && !self.world.is_poisoned() {
            self.remove(id);
            self.dropped.push(id);
        }
        made
    }

    /// Makes prop `id` a part of `root`'s body at `local` (a pose in `root`'s frame), as it is a child of `root` or of a part of
    /// it. False (nothing changed) when `root` is not a dynamic prop, `id` has joints or is `root`, the tick's budget for bodies
    /// is spent, or the world refuses it. The root is woken: its mass changed, and so is the mass of the root it left.
    pub fn attach(&mut self, id: u32, root: u32, local: Pose) -> bool {
        if id == root || !self.is_dynamic(root) || self.joints_on(id) > 0 {
            return false;
        }
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        let was = p.link;
        if matches!(was, Link::Part { root: r, .. } if r == root) {
            return self.move_part(id, local);
        }
        if self.build_exhausted() {
            return false;
        }
        // What hangs on this prop leaves its body with it, to be placed on the new root by the next fit.
        self.free_parts(id, false);
        let Some(s) = self.leave(id) else {
            return false;
        };
        if !self.put_part(id, root, local, true) {
            self.restore_form(id, was, &s);
            return false;
        }
        self.set_link(id, Link::Part { root, local });
        let placed = self
            .props
            .get(&root)
            .and_then(|r| part_pose(&r.q, local))
            .map(|(position, rotation)| rest_state(position, rotation));
        if let Some(p) = self.props.get_mut(&id) {
            p.rest = super::Rest::Awake;
            p.quiet = 0;
            if let Some(q) = placed {
                p.q = q;
            }
        }
        if let Link::Part { root: old, .. } = was {
            self.rouse(old);
        }
        self.rouse(root);
        self.changed.push((id, MASK_STATE | MASK_FLAGS));
        true
    }

    /// Moves part `id` to `local` in its root's frame (a script set its local pose). The only call that changes a part's pose.
    pub fn move_part(&mut self, id: u32, local: Pose) -> bool {
        let Some(Link::Part { root, .. }) = self.link(id) else {
            return false;
        };
        let moved = self.world.move_part(BodyId(id), local.0, local.1);
        let ok = moved.is_ok();
        self.note(&format!("prop {id}'s part"), moved);
        if ok {
            self.set_link(id, Link::Part { root, local });
            self.rouse(root);
        }
        ok
    }

    /// Makes prop `id` a driven child at `pose`, or takes a driven one to `pose` over the next step. False (nothing changed) for a
    /// prop with joints, a pose the world refuses, or when the tick's budget for bodies is spent.
    pub fn drive(&mut self, id: u32, pose: Pose) -> bool {
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        let was = p.link;
        if let Link::Driven {
            linvel,
            angvel,
            at,
            was_frozen,
        } = was
        {
            // Still where it was taken, and at rest: nothing to do.
            if at == Some(pose) && linvel == [0.0; 3] && angvel == [0.0; 3] {
                return true;
            }
            let moved = self.world.set_kinematic_target(BodyId(id), pose.0, pose.1);
            let ok = moved.is_ok();
            self.note(&format!("prop {id}'s drive"), moved);
            if let Some(s) = self.world.body(BodyId(id)) {
                self.set_link(
                    id,
                    Link::Driven {
                        linvel: s.linvel,
                        angvel: s.angvel,
                        at: ok.then_some(pose),
                        was_frozen,
                    },
                );
            }
            return ok;
        }
        if self.joints_on(id) > 0 || self.build_exhausted() {
            return false;
        }
        let was_frozen = p.rest.is_frozen();
        // A body hung on something that is no prop's body: what hung on it is driven where it is.
        self.free_parts(id, true);
        let Some(s) = self.leave(id) else {
            return false;
        };
        let state = BodyState {
            position: pose.0,
            rotation: pose.1,
            ..BodyState::default()
        };
        if !self.put_body(id, Motion::Kinematic, &state, true) {
            self.restore_form(id, was, &s);
            return false;
        }
        self.set_link(id, Link::driven(was_frozen));
        if let Some(p) = self.props.get_mut(&id) {
            p.rest = super::Rest::Awake;
            p.quiet = 0;
            p.q = Quantized::of(&state);
        }
        if let Link::Part { root, .. } = was {
            self.rouse(root);
        }
        self.changed.push((id, MASK_STATE | MASK_FLAGS));
        true
    }

    /// Makes child `id` a dynamic body again, at the pose it has and the speed its place gave it (the point of its parent's body it
    /// was a part of, or the last move that drove it); a prop that was frozen when it was hung is frozen again if its datablock
    /// still lets it. False for a prop that is no child or when the tick's budget for bodies is spent.
    pub fn release(&mut self, id: u32) -> bool {
        let Some(p) = self.props.get(&id) else {
            return false;
        };
        let was = p.link;
        let (root, refreeze) = match was {
            Link::Part { root, .. } => (Some(root), false),
            Link::Driven { was_frozen, .. } => (None, was_frozen && p.params.can_freeze),
            Link::Free => return false,
        };
        if self.build_exhausted() {
            return false;
        }
        let Some(s) = self.leave(id) else {
            return false;
        };
        let s = if refreeze {
            BodyState {
                linvel: [0.0; 3],
                angvel: [0.0; 3],
                sleeping: false,
                ..s
            }
        } else {
            BodyState {
                sleeping: false,
                ..s
            }
        };
        let motion = if refreeze {
            Motion::Fixed
        } else {
            Motion::Dynamic
        };
        if !self.put_body(id, motion, &s, true) {
            self.restore_form(id, was, &s);
            return false;
        }
        self.set_link(id, Link::Free);
        if let Some(p) = self.props.get_mut(&id) {
            p.rest = if refreeze {
                super::Rest::Frozen
            } else {
                super::Rest::Awake
            };
            p.quiet = 0;
            p.q = Quantized::of(&s);
        }
        if let Some(root) = root {
            self.rouse(root);
        }
        self.changed.push((id, MASK_STATE | MASK_FLAGS));
        true
    }

    /// Frees each part of `root` before its body is replaced or removed, as a driven child where it is when `driven` (its root is
    /// to be frozen or hung on something) or a dynamic body leaving at the speed its point had.
    pub(super) fn free_parts(&mut self, root: u32, driven: bool) {
        // The body is about to go: its parts must leave it whatever the tick's budget for bodies says.
        let spent = std::mem::take(&mut self.build_spent);
        self.free_each_part(root, driven);
        self.build_spent = self.build_spent.saturating_add(spent);
    }

    fn free_each_part(&mut self, root: u32, driven: bool) {
        for part in self.parts_of(root) {
            if driven {
                let at = self
                    .world
                    .part_world(BodyId(part))
                    .map(|s| (s.position, s.rotation));
                if let Some(at) = at {
                    self.drive(part, at);
                    continue;
                }
            }
            self.release(part);
        }
    }

    /// After the step: each child's state at the wire's precision, from where the world has it (a part's from its root's
    /// state), at rest. The ones whose state changed are listed in the report, for the tick to write quietly: a child is drawn
    /// from its parent, so its own moves send nothing. The parts of a root that did not move since their last settle are not
    /// looked at.
    pub(super) fn settle_children(&mut self, report: &mut TickReport) {
        let mut moved: Vec<(u32, Quantized)> = Vec::new();
        for id in self.kids.driven.iter().copied() {
            let pose = self
                .world
                .body(BodyId(id))
                .map(|s| (s.position, s.rotation));
            if let Some(q) = pose.map(|(position, rotation)| rest_state(position, rotation)) {
                moved.push((id, q));
            }
        }
        let stale: Vec<u32> = self
            .kids
            .parts
            .iter()
            .filter(|(root, group)| {
                group.seen.is_none() || group.seen != self.props.get(root).map(|r| r.q)
            })
            .map(|(&root, _)| root)
            .collect();
        for root in stale {
            let Some(root_q) = self.props.get(&root).map(|r| r.q) else {
                continue;
            };
            let ids = self.parts_of(root);
            for id in ids {
                let Some(Link::Part { local, .. }) = self.link(id) else {
                    continue;
                };
                if let Some((position, rotation)) = part_pose(&root_q, local) {
                    moved.push((id, rest_state(position, rotation)));
                }
            }
            if let Some(group) = self.kids.parts.get_mut(&root) {
                group.seen = Some(root_q);
            }
        }
        moved.sort_unstable_by_key(|&(id, _)| id);
        for (id, q) in moved {
            if let Some(p) = self.props.get_mut(&id)
                && p.q != q
            {
                p.q = q;
                self.revision = super::next_revision();
                report.moved.push(id);
            }
        }
    }
}

/// A child's state at rest at a pose, at the wire's precision.
fn rest_state(position: [f32; 3], rotation: [f32; 4]) -> Quantized {
    Quantized::of(&BodyState {
        position,
        rotation,
        ..BodyState::default()
    })
}
