//! `writePacketData` of a driven vehicle (the Player's control-object tail): `Vehicle::writePacketData` (FlyingVehicle's
//! packet hook forwards to it) and `WheeledVehicle::writePacketData`, as plain words. The host serialises them.
//!
//! ```text
//! F32 energy (getEnergyLevel)   F32 steering x   F32 steering y
//! 3 x F32 rigid position   Q4 orientation   3 x F32 linear momentum
//! 3 x F32 angular momentum   flag(at rest)   flag(contact count == 0)   flag(disableMove)
//! Wheeled: flag(brake), then per wheel (the datablock's count): F32 angular velocity, F32 dy, F32 dx, flag(slipping)
//! ```
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/vehicles/vehicle.cpp`,
//! `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/flyingVehicle.cpp` (each Copyright (c) 2012 GarageGames, LLC); the code
//! here is this crate's own.

use torque_collide::math::F3;

use crate::math::Quat;

/// One wheel's words: the angular velocity, the longitudinal and lateral tire deformations, slipping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelWords {
    pub avel: f32,
    pub dy: f32,
    pub dx: f32,
    pub slipping: bool,
}

/// What a vehicle class adds.
#[derive(Debug, Clone, PartialEq)]
pub enum KindWords {
    Flying,
    Wheeled {
        brake: bool,
        wheels: Vec<WheelWords>,
    },
}

/// The words of a driven vehicle's packet data, in write order.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleWords {
    pub energy: f32,
    pub steering: [f32; 2],
    pub position: F3,
    pub rotation: Quat,
    pub lin_mom: F3,
    pub ang_mom: F3,
    pub at_rest: bool,
    /// `flag(contact count == 0)`.
    pub contacts_zero: bool,
    pub disable_move: bool,
    pub kind: KindWords,
}

impl VehicleWords {
    /// The words as bit patterns in write order (flags as 0/1), for comparisons.
    pub fn bits(&self) -> Vec<u32> {
        let mut v = vec![
            self.energy.to_bits(),
            self.steering[0].to_bits(),
            self.steering[1].to_bits(),
        ];
        v.extend(self.position.map(f32::to_bits));
        v.extend(self.rotation.map(f32::to_bits));
        v.extend(self.lin_mom.map(f32::to_bits));
        v.extend(self.ang_mom.map(f32::to_bits));
        v.extend([
            u32::from(self.at_rest),
            u32::from(self.contacts_zero),
            u32::from(self.disable_move),
        ]);
        if let KindWords::Wheeled { brake, wheels } = &self.kind {
            v.push(u32::from(*brake));
            for w in wheels {
                v.extend([
                    w.avel.to_bits(),
                    w.dy.to_bits(),
                    w.dx.to_bits(),
                    u32::from(w.slipping),
                ]);
            }
        }
        v
    }
}

/// The base part of a state.
pub fn base_words(
    s: &crate::state::VehicleState,
    db: &crate::data::ShapeBaseData,
    host: &mut dyn crate::host::VehicleHost,
    kind: KindWords,
) -> VehicleWords {
    let r = &s.rigid;
    VehicleWords {
        energy: s.energy_level(db, host),
        steering: s.steering,
        position: r.lin_pos,
        rotation: r.ang_pos,
        lin_mom: r.lin_mom,
        ang_mom: r.ang_mom,
        at_rest: r.at_rest,
        contacts_zero: s.contacts_count == 0,
        disable_move: s.disable_move,
        kind,
    }
}

/// The ghost packer's 9-bit steering ints (`Vehicle::packUpdate`, for the ghosts other than the controlling client's; the
/// packet data carries the raw floats) (through the [`library`](crate::library) table): x then y, in write order.
pub fn steering_ints(steering: [f32; 2], max_steering_angle: f32) -> [u32; 2] {
    (crate::library::get().steering_ints)(steering, max_steering_angle)
}

/// Each steering component mapped from `[-max, max]` to `[0, 1]` and written as a 9-bit fraction (`writeFloat(f, 9)`: the
/// integer `f * 511`, cut toward zero, its low 9 bits). [`PLAIN`](crate::library::PLAIN)'s `steering_ints`.
pub fn plain_steering_ints(steering: [f32; 2], max_steering_angle: f32) -> [u32; 2] {
    let q = |x: f32| {
        let f = (x + max_steering_angle) / (2.0 * max_steering_angle);
        (torque_collide::math::float_to_int(f64::from(f * 511.0)) as u32) & 0x1ff
    };
    [q(steering[0]), q(steering[1])]
}
