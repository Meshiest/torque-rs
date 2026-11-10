//! Joints between bodies (Rapier impulse joints) by [`JointId`]: a distance range with an optional spring past it, a fixed
//! weld, and a revolute hinge with optional angle limits and a motor. An end is a body at a point in its own frame or a point in
//! the world (a hidden fixed body at the origin holds those).
//!
//! Joints are kept in a `BTreeMap` by id and inserted in the order they are added: add them in id order (a world made again
//! from a record adds every joint after every body, in id order). Every value is checked before Rapier sees it, as a body's
//! are. A body removed takes its joints with it.

use rapier3d::prelude::{
    FixedJointBuilder, GenericJoint, ImpulseJointHandle, JointAxis, MotorModel, Pose,
    RevoluteJointBuilder, RigidBodyBuilder, RopeJointBuilder,
};

use super::{Error, World, quat, v3};
use crate::guard;
use crate::world::BodyId;

/// A joint's id: the caller's object id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JointId(pub u32);

/// The longest a distance joint's range reaches (units): a longer one is held to it.
pub const MAX_DISTANCE: f32 = 10_000.0;
/// The shortest a distance joint's longest may be (Rapier wants more than zero).
pub const MIN_DISTANCE: f32 = 1.0e-3;
/// The largest spring stiffness (force a unit), spring damping (force a unit a second), motor speed (radians a second) and
/// motor torque a joint takes.
pub const MAX_STIFFNESS: f32 = 1.0e7;
pub const MAX_DAMPING: f32 = 1.0e6;
pub const MAX_MOTOR_SPEED: f32 = 100.0;
pub const MAX_MOTOR_TORQUE: f32 = 1.0e8;

/// One end of a joint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// A body, at `local` in its own frame.
    Body { body: BodyId, local: [f32; 3] },
    /// A point in the world.
    World([f32; 3]),
}

/// A spring past a distance joint's longest: it pulls the ends back with `stiffness` times the stretch and `damping` times
/// the speed apart (forces), and pushes nothing when they are within reach.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
}

/// A hinge's motor: it turns the second body about the axis at `speed` (radians a second), with at most `max_torque`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motor {
    pub speed: f32,
    pub max_torque: f32,
}

/// What a joint holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointKind {
    /// The anchors at least `min` and at most `max` apart; with a spring, `max` is where the spring starts and only `min` is
    /// held hard.
    Distance {
        min: f32,
        max: f32,
        spring: Option<Spring>,
    },
    /// The second body held at `frame` (a position and a rotation `[x, y, z, w]`) in the first's frame; the anchors are not used.
    Fixed { frame: ([f32; 3], [f32; 4]) },
    /// The anchors held together, the bodies free to turn only about the axis (`axis_a` in the first's frame, `axis_b` in the
    /// second's), within `limits` (radians, low then high) when there are some, driven by `motor` when there is one.
    Revolute {
        axis_a: [f32; 3],
        axis_b: [f32; 3],
        limits: Option<[f32; 2]>,
        motor: Option<Motor>,
    },
}

/// A joint to add.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointDesc {
    pub a: Anchor,
    pub b: Anchor,
    pub kind: JointKind,
}

/// A joint's state after the last step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct JointState {
    /// The distance between the anchors.
    pub distance: f32,
    /// The force and the torque the joint held its bodies with in the last solver iteration (its impulses over the iteration's
    /// seconds; a motor pushing against its own limit counts as the difference).
    pub force: f32,
    pub torque: f32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct JointEntry {
    handle: ImpulseJointHandle,
    desc: JointDesc,
    /// A soft distance joint's spring is pulling (its ends were past its reach before the last sub-step).
    spring_on: bool,
}

impl JointEntry {
    /// Whether the joint has `id` at an end.
    pub(super) fn holds(&self, id: BodyId) -> bool {
        [self.desc.a, self.desc.b]
            .iter()
            .any(|a| matches!(a, Anchor::Body { body, .. } if *body == id))
    }

    /// A soft distance joint's spring and longest reach.
    pub(super) fn spring(&self) -> Option<(f32, Spring)> {
        match self.desc.kind {
            JointKind::Distance {
                max,
                spring: Some(s),
                ..
            } => Some((max, s)),
            _ => None,
        }
    }
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let v = guard::magnitude(v, 1.0e6)?;
    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (len > 1.0e-6).then(|| v.map(|x| x / len))
}

/// A desc with every value checked and held to its range; None when one is not finite (or an axis is zero).
fn held(desc: &JointDesc, max_coordinate: f32) -> Option<JointDesc> {
    let anchor = |a: Anchor| match a {
        Anchor::Body { body, local } => Some(Anchor::Body {
            body,
            local: guard::coords(local, max_coordinate)?,
        }),
        Anchor::World(p) => Some(Anchor::World(guard::coords(p, max_coordinate)?)),
    };
    let kind = match desc.kind {
        JointKind::Distance { min, max, spring } => {
            let max = guard::scalar(max, MIN_DISTANCE, MAX_DISTANCE)?;
            let min = guard::scalar(min, 0.0, max)?;
            let spring = match spring {
                Some(s) => Some(Spring {
                    stiffness: guard::scalar(s.stiffness, 0.0, MAX_STIFFNESS)?,
                    damping: guard::scalar(s.damping, 0.0, MAX_DAMPING)?,
                }),
                None => None,
            };
            JointKind::Distance { min, max, spring }
        }
        JointKind::Fixed { frame: (p, q) } => JointKind::Fixed {
            frame: (guard::coords(p, max_coordinate)?, guard::rotation(q)?),
        },
        JointKind::Revolute {
            axis_a,
            axis_b,
            limits,
            motor,
        } => {
            let limits = match limits {
                Some([lo, hi]) => {
                    let lo = guard::scalar(lo, -std::f32::consts::PI, std::f32::consts::PI)?;
                    let hi = guard::scalar(hi, lo, std::f32::consts::PI)?;
                    Some([lo, hi])
                }
                None => None,
            };
            let motor = match motor {
                Some(m) => Some(Motor {
                    speed: guard::scalar(m.speed, -MAX_MOTOR_SPEED, MAX_MOTOR_SPEED)?,
                    max_torque: guard::scalar(m.max_torque, 0.0, MAX_MOTOR_TORQUE)?,
                }),
                None => None,
            };
            JointKind::Revolute {
                axis_a: unit(axis_a)?,
                axis_b: unit(axis_b)?,
                limits,
                motor,
            }
        }
    };
    Some(JointDesc {
        a: anchor(desc.a)?,
        b: anchor(desc.b)?,
        kind,
    })
}

/// The anchor's point in its body's frame (a world point is in the ground body's frame, which is the world's).
fn local(a: &Anchor) -> [f32; 3] {
    match *a {
        Anchor::Body { local, .. } => local,
        Anchor::World(p) => p,
    }
}

/// Rapier's joint for `desc` (held), its spring off.
fn rapier_joint(desc: &JointDesc) -> GenericJoint {
    let (la, lb) = (v3(local(&desc.a)), v3(local(&desc.b)));
    match desc.kind {
        JointKind::Distance { min, max, spring } => {
            let top = if spring.is_some() { MAX_DISTANCE } else { max };
            let mut j: GenericJoint = RopeJointBuilder::new(top)
                .local_anchor1(la)
                .local_anchor2(lb)
                .contacts_enabled(true)
                .build()
                .into();
            j.set_limits(JointAxis::LinX, [min, top]);
            if spring.is_some() {
                // Off until the ends are past the reach: the step turns it on and off before each sub-step.
                j.set_motor_model(JointAxis::LinX, MotorModel::ForceBased);
                // A motor of no gain still holds the speed apart at zero: off is no force at all.
                j.set_motor_position(JointAxis::LinX, max, 0.0, 0.0);
                j.set_motor_max_force(JointAxis::LinX, 0.0);
            }
            j
        }
        JointKind::Fixed { frame: (p, q) } => FixedJointBuilder::new()
            .local_frame1(Pose::from_parts(v3(p), quat(q)))
            .local_frame2(Pose::IDENTITY)
            .build()
            .into(),
        JointKind::Revolute {
            axis_a,
            axis_b,
            limits,
            motor,
        } => {
            let mut b = RevoluteJointBuilder::new(v3(axis_a))
                .local_anchor1(la)
                .local_anchor2(lb);
            if let Some(l) = limits {
                b = b.limits(l);
            }
            if let Some(m) = motor {
                // A force-based velocity motor pushes with its damping factor times how far the hinge's speed is from
                // `speed`, at most `max_torque`. A factor of the torque cap reaches the cap at 1 radian a second off the
                // speed: the motor holds its speed firmly under load without being a constant torque. At least 1, so a
                // motor with a tiny cap still has a gain.
                b = b
                    .motor_model(MotorModel::ForceBased)
                    .motor_velocity(m.speed, m.max_torque.max(1.0))
                    .motor_max_force(m.max_torque);
            }
            let mut j: GenericJoint = b.build().into();
            j.set_local_axis2(v3(axis_b));
            j
        }
    }
}

impl World {
    /// The hidden fixed body world points hang from, made on first use.
    fn ground(&mut self) -> Result<rapier3d::prelude::RigidBodyHandle, Error> {
        if let Some(g) = self.ground {
            return Ok(g);
        }
        let g = self.guarded(|r| r.insert_body(RigidBodyBuilder::fixed()))?;
        self.ground = Some(g);
        Ok(g)
    }

    fn end_handle(&mut self, a: &Anchor) -> Result<rapier3d::prelude::RigidBodyHandle, Error> {
        match a {
            Anchor::Body { body, .. } => self
                .bodies
                .get(body)
                .map(|e| e.handle)
                .ok_or(Error::Unknown(*body)),
            Anchor::World(_) => self.ground(),
        }
    }

    /// Adds a joint. Its values are checked (a non-finite one refuses the call) and held to their ranges; both ends may not be
    /// world points, nor one body twice. Refused past [`crate::Limits::max_joints`].
    pub fn add_joint(&mut self, id: JointId, desc: &JointDesc) -> Result<(), Error> {
        self.live()?;
        if self.joints.contains_key(&id) {
            return Err(Error::DuplicateJoint(id));
        }
        if self.joints.len() >= self.config.limits.max_joints {
            return Err(Error::Cap(crate::world::Cap::Joints));
        }
        let desc = held(desc, self.config.limits.max_coordinate).ok_or(Error::NonFinite)?;
        match (desc.a, desc.b) {
            (Anchor::World(_), Anchor::World(_)) => return Err(Error::NonFinite),
            (Anchor::Body { body: x, .. }, Anchor::Body { body: y, .. }) if x == y => {
                return Err(Error::Duplicate(x));
            }
            _ => {}
        }
        let (ha, hb) = (self.end_handle(&desc.a)?, self.end_handle(&desc.b)?);
        let joint = rapier_joint(&desc);
        let handle = self.guarded(|r| r.impulse_joints.insert(ha, hb, joint, true))?;
        self.joints.insert(
            id,
            JointEntry {
                handle,
                desc,
                spring_on: false,
            },
        );
        Ok(())
    }

    /// Changes what a joint holds (its kind's values; the ends stay). Wakes its bodies.
    pub fn set_joint(&mut self, id: JointId, kind: JointKind) -> Result<(), Error> {
        self.live()?;
        let e = *self.joints.get(&id).ok_or(Error::UnknownJoint(id))?;
        let desc = held(
            &JointDesc { kind, ..e.desc },
            self.config.limits.max_coordinate,
        )
        .ok_or(Error::NonFinite)?;
        let joint = rapier_joint(&desc);
        self.guarded(|r| {
            if let Some(j) = r.impulse_joints.get_mut(e.handle, true) {
                j.data = joint;
            }
        })?;
        if let Some(entry) = self.joints.get_mut(&id) {
            entry.desc = desc;
            entry.spring_on = false;
        }
        Ok(())
    }

    /// Removes a joint; true when it was there.
    pub fn remove_joint(&mut self, id: JointId) -> Result<bool, Error> {
        self.live()?;
        let Some(e) = self.joints.remove(&id) else {
            return Ok(false);
        };
        self.guarded(|r| {
            r.impulse_joints.remove(e.handle, true);
        })?;
        Ok(true)
    }

    /// A joint as it was added (held to its ranges).
    pub fn joint_desc(&self, id: JointId) -> Option<JointDesc> {
        self.joints.get(&id).map(|e| e.desc)
    }

    /// The joints' ids, ascending.
    pub fn joint_ids(&self) -> impl Iterator<Item = JointId> + '_ {
        self.joints.keys().copied()
    }

    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// An anchor's point in the world now.
    pub fn anchor_point(&self, a: &Anchor) -> Option<[f32; 3]> {
        match *a {
            Anchor::World(p) => Some(p),
            Anchor::Body { body, local } => {
                let e = self.bodies.get(&body)?;
                let b = self.rapier.bodies.get(e.handle)?;
                let p = b.position().transform_point(v3(local));
                Some([p.x, p.y, p.z])
            }
        }
    }

    /// A joint's state after the last step.
    pub fn joint(&self, id: JointId) -> Option<JointState> {
        let e = self.joints.get(&id)?;
        let j = self.rapier.impulse_joints.get(e.handle)?;
        let (pa, pb) = (self.anchor_point(&e.desc.a)?, self.anchor_point(&e.desc.b)?);
        let d: [f32; 3] = std::array::from_fn(|k| pb[k] - pa[k]);
        // The impulses are the last solver iteration's, and each iteration is a sub-step of its own.
        let p = &self.rapier.integration_parameters;
        let dt = p.dt / p.num_solver_iterations.max(1) as f32;
        // Rapier keeps a locked axis's impulse, a limit's and a motor's apart: each axis's is their sum.
        let axis: [f32; 6] = std::array::from_fn(|k| {
            j.impulses[k] + j.data.limits[k].impulse + j.data.motors[k].impulse
        });
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let (lin, ang) = (norm(&axis[..3]), norm(&axis[3..]));
        // A solver driven past its range (hostile limits or springs) can leave impulses that are not numbers: they read as the
        // most there is, so a joint with a break force breaks.
        let finite = |v: f32| if v.is_finite() { v } else { f32::MAX };
        Some(JointState {
            distance: finite((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()),
            force: finite(lin / dt),
            torque: finite(ang / dt),
        })
    }

    /// A point of the world in a body's own frame (where a joint's end on it is anchored).
    pub fn to_local(&self, body: BodyId, point: [f32; 3]) -> Option<[f32; 3]> {
        let e = self.bodies.get(&body)?;
        let b = self.rapier.bodies.get(e.handle)?;
        let p = guard::coords(point, self.config.limits.max_coordinate)?;
        let l = b.position().inverse_transform_point(v3(p));
        Some([l.x, l.y, l.z])
    }

    /// A direction of the world in a body's own frame.
    pub fn to_local_dir(&self, body: BodyId, dir: [f32; 3]) -> Option<[f32; 3]> {
        let e = self.bodies.get(&body)?;
        let b = self.rapier.bodies.get(e.handle)?;
        let d = unit(dir)?;
        let l = b.position().inverse_transform_vector(v3(d));
        Some([l.x, l.y, l.z])
    }

    /// Where `b` is in `a`'s frame now (a position and a rotation `[x, y, z, w]`): a weld's frame. A missing body is the world's
    /// frame (the origin, unturned).
    pub fn relative_pose(
        &self,
        a: Option<BodyId>,
        b: Option<BodyId>,
    ) -> Option<([f32; 3], [f32; 4])> {
        let pose = |id: Option<BodyId>| -> Option<Pose> {
            match id {
                None => Some(Pose::IDENTITY),
                Some(id) => {
                    let e = self.bodies.get(&id)?;
                    Some(*self.rapier.bodies.get(e.handle)?.position())
                }
            }
        };
        let r = pose(a)?.inv_mul(&pose(b)?);
        let t = r.translation;
        Some(([t.x, t.y, t.z], r.rotation.to_array()))
    }

    /// Each soft distance joint's spring on when its ends are past its reach, off when they are not (before a sub-step). Only a
    /// joint whose state flips is touched.
    pub(super) fn tend_springs(&mut self) -> Result<(), Error> {
        let flips: Vec<(JointId, ImpulseJointHandle, f32, Spring, bool)> = self
            .joints
            .iter()
            .filter_map(|(&id, e)| {
                let (max, s) = e.spring()?;
                let pa = self.anchor_point(&e.desc.a)?;
                let pb = self.anchor_point(&e.desc.b)?;
                let d: [f32; 3] = std::array::from_fn(|k| pb[k] - pa[k]);
                let on = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > max;
                (on != e.spring_on).then_some((id, e.handle, max, s, on))
            })
            .collect();
        if flips.is_empty() {
            return Ok(());
        }
        self.guarded(|r| {
            for &(_, h, max, s, on) in &flips {
                let Some(j) = r.impulse_joints.get_mut(h, false) else {
                    continue;
                };
                let (k, c, most) = if on {
                    (s.stiffness, s.damping, f32::MAX)
                } else {
                    (0.0, 0.0, 0.0)
                };
                j.data.set_motor_position(JointAxis::LinX, max, k, c);
                j.data.set_motor_max_force(JointAxis::LinX, most);
            }
        })?;
        for (id, .., on) in flips {
            if let Some(e) = self.joints.get_mut(&id) {
                e.spring_on = on;
            }
        }
        Ok(())
    }

    /// The joints of the bodies a removal took away, forgotten (Rapier took them with the body).
    pub(super) fn forget_joints_of(&mut self, id: BodyId) {
        self.joints.retain(|_, e| !e.holds(id));
    }
}
