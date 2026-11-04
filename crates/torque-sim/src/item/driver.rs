//! A driver for the Item: a start state and the script-side actions between ticks in, the fields the Item's packer sends after
//! every tick out. A caller can feed it scripted actions (the throw's `applyImpulse`, a `setTransform`, ...) and compare each
//! tick's [`ItemTick`] with the expected one.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/item.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::ObjectId;
use torque_collide::math::{F3, Matrix};

use super::{Item, ItemData, ItemHost};

/// What a script does to an Item between two ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemAction {
    SetTransform(Matrix),
    SetVelocity(F3),
    ApplyImpulse(F3),
    SetCollisionTimeout(ObjectId),
}

/// One tick's outcome: the raw fields the packer writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemTick {
    pub position: F3,
    pub velocity: F3,
    pub at_rest: bool,
    pub rest_ticks: i32,
    pub transform: Matrix,
    pub mask_bits: u32,
    /// The work budget ran out inside the step (the state is as far as it got).
    pub exhausted: bool,
}

impl ItemTick {
    /// The words as bit patterns in packer order (position, the at-rest flag, the velocity), for comparisons.
    pub fn bits(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.position.iter().map(|x| x.to_bits()).collect();
        v.push(u32::from(self.at_rest));
        v.extend(self.velocity.iter().map(|x| x.to_bits()));
        v
    }
}

/// Runs `plan.len()` ticks from a copy of `start`: `plan[i]` are the actions applied before tick `i`. The host is the world.
pub fn run(
    start: &Item,
    db: &ItemData,
    plan: &[Vec<ItemAction>],
    host: &mut dyn ItemHost,
) -> Vec<ItemTick> {
    let mut it = start.clone();
    let mut out = Vec::with_capacity(plan.len());
    for actions in plan {
        it.s.mask_bits = 0;
        for a in actions {
            match *a {
                ItemAction::SetTransform(m) => it.set_transform(&m, host),
                ItemAction::SetVelocity(v) => it.set_velocity(v),
                ItemAction::ApplyImpulse(v) => it.apply_impulse(db, v),
                ItemAction::SetCollisionTimeout(o) => it.set_collision_timeout(o),
            }
        }
        let exhausted = it.process_tick(db, host).is_err();
        out.push(ItemTick {
            position: it.s.position(),
            velocity: it.s.velocity,
            at_rest: it.s.at_rest,
            rest_ticks: it.s.rest_ticks,
            transform: it.s.scene.obj_to_world,
            mask_bits: it.s.mask_bits,
            exhausted,
        });
    }
    out
}
