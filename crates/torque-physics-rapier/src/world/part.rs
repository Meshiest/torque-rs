//! Parts: colliders that sit on another body at a pose of their own, so a group of rigid pieces is one body with one mass. A part
//! has an id in the caller's own id space (a body's id and a part's never match), adds its mass to the body it is on, moves with
//! it and is read back as a pose in the body's frame, which nothing but the caller changes: the body's motion never touches it.

use std::collections::{BTreeMap, BTreeSet};

use rapier3d::prelude::{ColliderBuilder, ColliderHandle, Pose};

use crate::guard;
use crate::shape::{self, Shape};
use crate::work;

use super::{BodyDesc, BodyId, BodyState, Cap, Error, World, a3, quat, v3};

/// A part to add: a shape at a pose in the frame of the body it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct PartDesc {
    pub shape: Shape,
    /// Where the part's origin is in the body's frame.
    pub position: [f32; 3],
    /// A quaternion `[x, y, z, w]` (normalized on the way in).
    pub rotation: [f32; 4],
    /// Mass a cubic unit.
    pub density: f32,
    /// 0 to 10.
    pub friction: f32,
    /// 0 to 1.
    pub restitution: f32,
}

impl PartDesc {
    /// A part of `shape` at `position` and `rotation` in the body's frame, with a body's default surface.
    #[must_use]
    pub fn new(shape: Shape, position: [f32; 3], rotation: [f32; 4]) -> PartDesc {
        let body = BodyDesc::default();
        PartDesc {
            shape,
            position,
            rotation,
            density: body.density,
            friction: body.friction,
            restitution: body.restitution,
        }
    }
}

/// A part's place: the body it is on and its pose in that body's frame, as it was given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartState {
    pub onto: BodyId,
    pub position: [f32; 3],
    /// `[x, y, z, w]`.
    pub rotation: [f32; 4],
}

impl PartState {
    /// The place as words (each float by its bits), for a hash.
    #[must_use]
    pub fn words(&self) -> [u32; 8] {
        let floats = self.position.iter().chain(&self.rotation);
        let mut out = [0u32; 8];
        out[0] = self.onto.0;
        for (o, f) in out[1..].iter_mut().zip(floats) {
            *o = f.to_bits();
        }
        out
    }
}

/// The parts on one body: their ids, and their reaches as a multiset (reaches are never negative, so a float's bits order them), so
/// a body's reach is the last key and a part added, moved or removed costs `log n`.
#[derive(Debug, Default)]
pub(super) struct BodyParts {
    ids: BTreeSet<BodyId>,
    reaches: BTreeMap<u32, u32>,
}

impl BodyParts {
    fn add(&mut self, id: BodyId, reach: f32) {
        self.ids.insert(id);
        *self.reaches.entry(reach.to_bits()).or_default() += 1;
    }

    fn remove(&mut self, id: BodyId, reach: f32) {
        self.ids.remove(&id);
        if let Some(n) = self.reaches.get_mut(&reach.to_bits()) {
            *n -= 1;
            if *n == 0 {
                self.reaches.remove(&reach.to_bits());
            }
        }
    }

    fn reach(&self) -> f32 {
        self.reaches
            .last_key_value()
            .map_or(0.0, |(&bits, _)| f32::from_bits(bits))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PartEntry {
    onto: BodyId,
    collider: ColliderHandle,
    position: [f32; 3],
    rotation: [f32; 4],
    /// Where the shape sits about the part's origin (a hull is made about its middle).
    offset: [f32; 3],
    /// How far the part's shape reaches from its own origin, and from the body's.
    size: f32,
    reach: f32,
}

fn pose(position: [f32; 3], rotation: [f32; 4]) -> Pose {
    Pose::from_parts(v3(position), quat(rotation))
}

impl World {
    /// Adds a part `id` on body `onto`. Every value is checked first and held to the limits as a body's are; a hull that cannot
    /// be made becomes its box. The body's mass and center of mass take the part in.
    ///
    /// # Errors
    /// [`Error::Duplicate`] for an id that is a body or a part, [`Error::Unknown`] for an `onto` that is not a body,
    /// [`Error::Cap`] past the colliders cap, [`Error::NonFinite`] for a bad value, [`Error::Exhausted`] past the budget.
    pub fn add_part(
        &mut self,
        id: BodyId,
        onto: BodyId,
        desc: &PartDesc,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Error> {
        self.live()?;
        if self.bodies.contains_key(&id) || self.parts.contains_key(&id) {
            return Err(Error::Duplicate(id));
        }
        let host = *self.bodies.get(&onto).ok_or(Error::Unknown(onto))?;
        let l = self.config.limits;
        if self.rapier.colliders.len() >= l.max_colliders {
            return Err(Error::Cap(Cap::Colliders));
        }
        let position = guard::coords(desc.position, l.max_coordinate).ok_or(Error::NonFinite)?;
        let rotation = guard::rotation(desc.rotation).ok_or(Error::NonFinite)?;
        let density =
            guard::scalar(desc.density, l.min_density, l.max_density).ok_or(Error::NonFinite)?;
        let friction = guard::scalar(desc.friction, 0.0, 10.0).ok_or(Error::NonFinite)?;
        let restitution = guard::scalar(desc.restitution, 0.0, 1.0).ok_or(Error::NonFinite)?;
        if !charge(work::BODY + work::CONVEX) {
            return Err(Error::Exhausted);
        }
        let built = shape::build(&desc.shape, &l, charge)?;
        let at = pose(position, rotation) * Pose::from_translation(v3(built.offset));
        let collider = ColliderBuilder::new(built.shape)
            .position(at)
            .density(density)
            .friction(friction)
            .restitution(restitution)
            .build();
        let size = collider.shape().compute_local_bounding_sphere().radius
            + built.offset.iter().map(|v| v * v).sum::<f32>().sqrt();
        let reach = size + position.iter().map(|v| v * v).sum::<f32>().sqrt();
        let collider = self.guarded(|r| r.insert_collider(collider, Some(host.handle)))?;
        self.stats.hulls_replaced += usize::from(built.replaced);
        self.parts.insert(
            id,
            PartEntry {
                onto,
                collider,
                position,
                rotation,
                offset: built.offset,
                size,
                reach,
            },
        );
        self.part_index.entry(onto).or_default().add(id, reach);
        self.refresh_reach(onto);
        Ok(())
    }

    /// Takes a part off its body (its mass goes with it).
    ///
    /// # Errors
    /// [`Error::Unknown`] when there is no such part.
    pub fn remove_part(&mut self, id: BodyId) -> Result<(), Error> {
        self.live()?;
        let part = self.parts.remove(&id).ok_or(Error::Unknown(id))?;
        if let Some(index) = self.part_index.get_mut(&part.onto) {
            index.remove(id, part.reach);
            if index.ids.is_empty() {
                self.part_index.remove(&part.onto);
            }
        }
        self.refresh_reach(part.onto);
        self.guarded(|r| {
            r.remove_collider(part.collider);
        })
    }

    /// A part's place.
    #[must_use]
    pub fn part(&self, id: BodyId) -> Option<PartState> {
        let p = self.parts.get(&id)?;
        Some(PartState {
            onto: p.onto,
            position: p.position,
            rotation: p.rotation,
        })
    }

    /// The parts' ids, ascending.
    pub fn part_ids(&self) -> impl Iterator<Item = BodyId> + '_ {
        self.parts.keys().copied()
    }

    /// The parts on `onto`, ascending.
    pub fn parts_on(&self, onto: BodyId) -> impl Iterator<Item = BodyId> + '_ {
        self.part_index
            .get(&onto)
            .into_iter()
            .flat_map(|index| index.ids.iter().copied())
    }

    /// Moves a part to a new pose in its body's frame (checked as [`World::add_part`]'s are); it is the only call that changes a
    /// part's place.
    ///
    /// # Errors
    /// [`Error::Unknown`] when there is no such part, [`Error::NonFinite`] for a bad value.
    pub fn move_part(
        &mut self,
        id: BodyId,
        position: [f32; 3],
        rotation: [f32; 4],
    ) -> Result<(), Error> {
        self.live()?;
        let position =
            guard::coords(position, self.config.limits.max_coordinate).ok_or(Error::NonFinite)?;
        let rotation = guard::rotation(rotation).ok_or(Error::NonFinite)?;
        let part = *self.parts.get(&id).ok_or(Error::Unknown(id))?;
        let at = pose(position, rotation) * Pose::from_translation(v3(part.offset));
        self.guarded(|r| {
            if let Some(c) = r.colliders.get_mut(part.collider) {
                c.set_position_wrt_parent(at);
            }
        })?;
        let reach = part.size + position.iter().map(|v| v * v).sum::<f32>().sqrt();
        if let Some(p) = self.parts.get_mut(&id) {
            p.position = position;
            p.rotation = rotation;
            p.reach = reach;
        }
        if let Some(index) = self.part_index.get_mut(&part.onto) {
            index.remove(id, part.reach);
            index.add(id, reach);
        }
        self.refresh_reach(part.onto);
        Ok(())
    }

    /// Takes a part off its body as a body's state: the pose it has in the world and the velocity its body gives that point. The
    /// part is gone from the world; the caller makes a body of it (or not).
    ///
    /// # Errors
    /// [`Error::Unknown`] when there is no such part.
    pub fn detach_part(&mut self, id: BodyId) -> Result<BodyState, Error> {
        let state = self.part_world(id).ok_or(Error::Unknown(id))?;
        self.remove_part(id)?;
        Ok(state)
    }

    /// A part's pose in the world, with the velocity its body gives that point (its body's spin about the body's center of
    /// mass); asleep as its body is.
    #[must_use]
    pub fn part_world(&self, id: BodyId) -> Option<BodyState> {
        let part = self.parts.get(&id)?;
        let host = self.bodies.get(&part.onto)?;
        let body = self.rapier.bodies.get(host.handle)?;
        let at = *body.position() * pose(part.position, part.rotation);
        Some(BodyState {
            position: a3(at.translation),
            rotation: at.rotation.to_array(),
            linvel: a3(body.velocity_at_point(at.translation)),
            angvel: a3(body.angvel()),
            sleeping: body.is_sleeping(),
        })
    }

    /// The body's mass.
    #[must_use]
    pub fn mass_of(&self, id: BodyId) -> Option<f32> {
        let e = self.bodies.get(&id)?;
        Some(self.rapier.bodies.get(e.handle)?.mass())
    }

    /// The velocity of the point `point` (a world point) of body `id`, as its spin about its center of mass adds to its speed.
    #[must_use]
    pub fn velocity_at(&self, id: BodyId, point: [f32; 3]) -> Option<[f32; 3]> {
        let e = self.bodies.get(&id)?;
        let body = self.rapier.bodies.get(e.handle)?;
        Some(a3(body.velocity_at_point(v3(point))))
    }

    /// The parts as words, in id order (each one's [`PartState::words`]), for a hash.
    #[must_use]
    pub fn part_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.parts.len() * 9);
        for (&id, p) in &self.parts {
            out.push(id.0);
            out.extend(
                PartState {
                    onto: p.onto,
                    position: p.position,
                    rotation: p.rotation,
                }
                .words(),
            );
        }
        out
    }

    /// Body `id`'s reach: its own and its parts'.
    fn refresh_reach(&mut self, id: BodyId) {
        let parts = self.part_index.get(&id).map_or(0.0, BodyParts::reach);
        if let Some(e) = self.bodies.get_mut(&id) {
            e.radius = e.base_radius.max(parts);
        }
    }

    /// The parts of a body a removal took away, forgotten (Rapier took their colliders with it).
    pub(super) fn forget_parts_of(&mut self, id: BodyId) {
        if let Some(index) = self.part_index.remove(&id) {
            for part in index.ids {
                self.parts.remove(&part);
            }
        }
    }
}
