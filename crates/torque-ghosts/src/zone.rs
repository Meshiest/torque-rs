//! TGE's `PhysicalZone` (SceneObject-derived: no GameBase part; its own mask bits 0 and 1).
//!
//! ```text
//! flag(Initial, bit 0): F32 x16 (matrix), F32 x3 (scale);
//!     raw 32 count, per point F32 x3; raw 32 count, per plane F32 x4; raw 32 count, per edge raw 32 x4 (face 0, face 1,
//!     vertex 0, vertex 1); F32 velocity mod, F32 gravity mod, F32 x3 applied force
//! flag active                                                     (every update, after the initial block if any)
//! ```
//! The three counts are 32-bit words: the reader refuses one above [`WIRE_ZONE_POLYHEDRON`] or above what the bits left can hold.
//!
//! **View fields:** `matrix` 0..16, `scale` 0..3, `point_count` and `point` (`3 i + k`), `plane_count` and `plane` (`4 i +
//! k`), `edge_count` and `edge` (`4 i + k`), `velocity_mod`, `gravity_mod`, `applied_force` 0..3, `active`.
//!
//! Credit: this follows TGE 1.4's `game/physicalZone.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/physicalZone.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The layout
//! is TGE's; the code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, opt};
use crate::props::matrix;
use crate::{ObjectView, boolean, int, raw, raws, word};

pub const ZONE_INITIAL_MASK: u32 = 1 << 0;
pub const ZONE_ACTIVE_MASK: u32 = 1 << 1;

/// The most points, planes or edges a zone's polyhedron may have on the wire (each list).
pub const WIRE_ZONE_POLYHEDRON: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZoneInitial {
    pub matrix: [F32; 16],
    pub scale: [F32; 3],
    pub points: Vec<[F32; 3]>,
    pub planes: Vec<[F32; 4]>,
    /// Face 0, face 1, vertex 0, vertex 1.
    pub edges: Vec<[u32; 4]>,
    pub velocity_mod: F32,
    pub gravity_mod: F32,
    pub applied_force: [F32; 3],
}

/// `PhysicalZone`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhysicalZone {
    pub initial: Option<ZoneInitial>,
    pub active: bool,
}

impl Layout for PhysicalZone {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "zone.initial?", &mut self.initial, |c, z| {
            c.floats("zone.matrix", &mut z.matrix)?;
            c.floats("zone.scale", &mut z.scale)?;
            c.seq(
                "zone.points",
                &mut z.points,
                32,
                WIRE_ZONE_POLYHEDRON,
                96,
                |c, p| c.floats("zone.point", p),
            )?;
            c.seq(
                "zone.planes",
                &mut z.planes,
                32,
                WIRE_ZONE_POLYHEDRON,
                128,
                |c, p| c.floats("zone.plane", p),
            )?;
            c.seq(
                "zone.edges",
                &mut z.edges,
                32,
                WIRE_ZONE_POLYHEDRON,
                128,
                |c, e| {
                    for v in e.iter_mut() {
                        c.raw32("zone.edge", v)?;
                    }
                    Ok(())
                },
            )?;
            c.raw32("zone.velocityMod", &mut z.velocity_mod.0)?;
            c.raw32("zone.gravityMod", &mut z.gravity_mod.0)?;
            c.floats("zone.appliedForce", &mut z.applied_force)
        })?;
        c.flag("zone.active", &mut self.active)
    }
}

fn count(view: &dyn ObjectView, field: &str) -> usize {
    (int(view, field, 0).max(0) as usize).min(WIRE_ZONE_POLYHEDRON)
}

pub fn pack_physical_zone(view: &dyn ObjectView, mask: u32) -> PhysicalZone {
    let initial = (mask & ZONE_INITIAL_MASK != 0).then(|| ZoneInitial {
        matrix: matrix(view),
        scale: raws::<3>(view, "scale", 0),
        points: (0..count(view, "point_count"))
            .map(|i| raws::<3>(view, "point", 3 * i))
            .collect(),
        planes: (0..count(view, "plane_count"))
            .map(|i| raws::<4>(view, "plane", 4 * i))
            .collect(),
        edges: (0..count(view, "edge_count"))
            .map(|i| std::array::from_fn(|k| word(view, "edge", 4 * i + k)))
            .collect(),
        velocity_mod: raw(view, "velocity_mod", 0),
        gravity_mod: raw(view, "gravity_mod", 0),
        applied_force: raws::<3>(view, "applied_force", 0),
    });
    PhysicalZone {
        initial,
        active: boolean(view, "active", 0),
    }
}
