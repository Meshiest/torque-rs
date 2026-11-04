//! A driver for the Projectile: a start state in, the fields the Projectile's packer sends after every tick out; the callbacks
//! the host saw are kept by the host itself.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/projectile.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::F3;

use super::{Projectile, ProjectileData, ProjectileHost};

/// One tick's outcome: the raw fields the packer writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileTick {
    pub position: F3,
    pub velocity: F3,
    pub age: u32,
    pub exploded: bool,
    pub explosion_pos: F3,
    pub explosion_normal: F3,
    pub mask_bits: u32,
}

impl ProjectileTick {
    /// The words as bit patterns in packer order (position, velocity, the explosion), for comparisons.
    pub fn bits(&self) -> Vec<u32> {
        let mut v: Vec<u32> = Vec::new();
        for a in [
            self.position,
            self.velocity,
            self.explosion_pos,
            self.explosion_normal,
        ] {
            v.extend(a.iter().map(|x| x.to_bits()));
        }
        v.push(self.age);
        v.push(u32::from(self.exploded));
        v.push(self.mask_bits);
        v
    }
}

/// Runs `ticks` ticks from a copy of `start`, clearing the mask bits before each. A projectile the host deleted keeps ticking
/// here (the host's deletes are events it records).
pub fn run(
    start: &Projectile,
    db: &ProjectileData,
    ticks: usize,
    host: &mut dyn ProjectileHost,
) -> Vec<ProjectileTick> {
    let mut p = start.clone();
    let mut out = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        p.s.mask_bits = 0;
        p.process_tick(db, host);
        let s = &p.s;
        out.push(ProjectileTick {
            position: s.position,
            velocity: s.velocity,
            age: s.age,
            exploded: s.exploded,
            explosion_pos: s.explosion_pos,
            explosion_normal: s.explosion_normal,
            mask_bits: s.mask_bits,
        });
    }
    out
}
