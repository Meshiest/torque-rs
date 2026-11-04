//! TGE's vehicles: the Vehicle part ([`VehiclePart`], over [`TgeShapeBase`]) and `WheeledVehicle` (and `AIWheeledVehicle`,
//! which sends its record), `FlyingVehicle` and `HoverVehicle`.
//!
//! ```text
//! Vehicle part:   ShapeBase part; flag jetting; flag(controlled by this connection && !initial): the part ends;
//!                 int(9) steering yaw, int(9) steering pitch (writeFloat of (steering + max) / (2 max)); the move (Move::pack);
//!                 flag(Position, bit 25): compressed point (position), F32 x4 rotation (quaternion), F32 x3 linear momentum,
//!                 F32 x3 angular momentum, flag at rest;
//!                 flag(Energy, bit 26, && a passenger is this connection's and does not control the vehicle): int(8) energy
//! WheeledVehicle: Vehicle part; flag(Wheel, bit 27): per wheel flag(tire && spring): ranged(tire id), ranged(spring id),
//!                 flag powered, F32 steering;
//!                 flag(controlled by this connection && !initial): the record ends;
//!                 flag braking; flag(Position): per wheel F32 angular velocity, F32 Dy, F32 Dx
//! FlyingVehicle:  Vehicle part; unless controlled by this connection (and not an initial update): flag createHeightOn,
//!                 int(3) thrust direction. There is no flag: the reader reads the two unless it controls the vehicle.
//! HoverVehicle:   Vehicle part; int(3) thrust direction
//! ```
//! A WheeledVehicle's wheel count is its datablock's (`WheeledVehicleData::wheelCount`, at most 8): the reader asks its
//! [`crate::ClientFacts`] for it.
//!
//! **View fields:** the ShapeBase part's; `jetting`, `controlled_here`, `steering` 0..2, `max_steering_angle`, the move
//! ([`PackedMove::from_view`]), `position` 0..3, `rotation` 0..4 (a quaternion x, y, z, w), `linear_momentum` 0..3,
//! `angular_momentum` 0..3, `at_rest`, `passenger_here` (a mounted object this connection controls, the vehicle not being its
//! control object), `energy_value` (energy over its maximum); WheeledVehicle: `wheel_count`, per wheel `wheel_tire`,
//! `wheel_spring`, `wheel_powered`, `wheel_steering`, `wheel_avel`, `wheel_dy`, `wheel_dx`, and `braking`; FlyingVehicle:
//! `create_height_on`, `thrust_direction`; HoverVehicle: `thrust_direction`.
//!
//! Credit: these follow TGE 1.4's `game/vehicles/vehicle.cc`, `wheeledVehicle.cc`, `flyingVehicle.cc` and `hoverVehicle.cc`
//! (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successors are Torque3D's `T3D/vehicles/vehicle.cpp`,
//! `wheeledVehicle.cpp`, `flyingVehicle.cpp` and `hoverVehicle.cpp` (each Copyright (c) 2012 GarageGames, LLC). The layouts
//! are TGE's (Torque3D's FlyingVehicle adds a flag before its two fields); the code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, WIRE_WHEELS, datablock_id, mask_bits, opt};
use crate::motion::{CPoint, PackedMove, float_bits};
use crate::parts::mask::INITIAL;
use crate::shapebase::{TgeShapeBase, pack_shapebase};
use crate::{ObjectView, PackCtx, boolean, fld, id, int, raw, raws};

/// The Vehicle part's mask bits and WheeledVehicle's.
pub mod mask {
    pub const POSITION: u32 = 1 << 25;
    pub const ENERGY: u32 = 1 << 26;
    pub const WHEEL: u32 = 1 << 27;
}

pub const STEERING_BITS: u8 = 9;
pub const VEHICLE_ENERGY_BITS: u8 = 8;
pub const THRUST_BITS: u8 = 3;

/// The rigid body's state in the position block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RigidState {
    pub position: CPoint,
    pub rotation: [F32; 4],
    pub linear_momentum: [F32; 3],
    pub angular_momentum: [F32; 3],
    pub at_rest: bool,
}

/// What the Vehicle part sends to a connection that does not control the vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VehicleState {
    /// Yaw and pitch.
    pub steering: [u32; 2],
    pub mv: PackedMove,
    pub rigid: Option<RigidState>,
    pub energy: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VehiclePart {
    pub shape: TgeShapeBase,
    pub jetting: bool,
    /// Controlled by this connection and not an initial update: the part ends.
    pub controlled: bool,
    pub state: Option<VehicleState>,
}

impl VehiclePart {
    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.flag("vehicle.jetting", &mut self.jetting)?;
        c.flag("vehicle.controlled", &mut self.controlled)?;
        if self.controlled {
            self.state = None;
            return Ok(());
        }
        let s = self.state.get_or_insert_with(VehicleState::default);
        c.int("vehicle.yaw", &mut s.steering[0], STEERING_BITS)?;
        c.int("vehicle.pitch", &mut s.steering[1], STEERING_BITS)?;
        s.mv.layout(c)?;
        opt(c, "vehicle.position?", &mut s.rigid, |c, r| {
            r.position.layout(c)?;
            c.floats("vehicle.rotation", &mut r.rotation)?;
            c.floats("vehicle.linearMomentum", &mut r.linear_momentum)?;
            c.floats("vehicle.angularMomentum", &mut r.angular_momentum)?;
            c.flag("vehicle.atRest", &mut r.at_rest)
        })?;
        opt(c, "vehicle.energy?", &mut s.energy, |c, e| {
            c.int("vehicle.energy", e, VEHICLE_ENERGY_BITS)
        })
    }

    /// The object's datablock as this record carries it (an initial update's), for the wheel count.
    fn datablock(&self) -> Option<u32> {
        self.shape.game.datablock
    }
}

/// Whether the packer stops after the controlled flag.
fn controlled(view: &dyn ObjectView, mask: u32) -> bool {
    boolean(view, "controlled_here", 0) && mask & INITIAL == 0
}

/// Packs the Vehicle part: the record and the return mask (the ShapeBase part's).
pub fn pack_vehicle_part(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (VehiclePart, u32) {
    let (shape, ret) = pack_shapebase(view, mask, ctx);
    let jetting = boolean(view, "jetting", 0);
    let controlled = controlled(view, mask);
    let state = (!controlled).then(|| {
        let max = fld(view, "max_steering_angle", 0);
        let steer = |i| {
            float_bits(
                (fld(view, "steering", i) + max) / (2.0 * max),
                STEERING_BITS,
            )
        };
        let energy = fld(view, "energy_value", 0).clamp(0.0, 1.0);
        VehicleState {
            steering: [steer(0), steer(1)],
            mv: PackedMove::from_view(view),
            rigid: (mask & self::mask::POSITION != 0).then(|| RigidState {
                position: CPoint::pack(
                    raws::<3>(view, "position", 0).map(F32::get),
                    ctx.compression_point,
                ),
                rotation: raws::<4>(view, "rotation", 0),
                linear_momentum: raws::<3>(view, "linear_momentum", 0),
                angular_momentum: raws::<3>(view, "angular_momentum", 0),
                at_rest: boolean(view, "at_rest", 0),
            }),
            energy: (mask & self::mask::ENERGY != 0 && boolean(view, "passenger_here", 0))
                .then(|| float_bits(energy, VEHICLE_ENERGY_BITS)),
        }
    });
    (
        VehiclePart {
            shape,
            jetting,
            controlled,
            state,
        },
        ret,
    )
}

/// The wheel count a list is laid out with: the client's (from the datablock) when decoding, the record's otherwise.
fn wheels<C: Codec>(
    c: &mut C,
    datablock: Option<u32>,
    record: usize,
) -> Result<usize, torque_net::NetError> {
    Ok(c.wheel_count(datablock)?.unwrap_or(record.min(WIRE_WHEELS)))
}

/// A wheel's tire and spring datablocks, power and steering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WheelSetup {
    pub tire: u32,
    pub spring: u32,
    pub powered: bool,
    pub steering: F32,
}

/// What a WheeledVehicle sends to a connection that does not control it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledMotion {
    pub braking: bool,
    /// Per wheel: angular velocity, Dy, Dx.
    pub wheels: Option<Vec<[F32; 3]>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledVehicle {
    pub vehicle: VehiclePart,
    /// Per wheel: `None` when it has no tire or spring.
    pub setup: Option<Vec<Option<WheelSetup>>>,
    pub controlled: bool,
    pub motion: Option<WheeledMotion>,
}

impl Layout for WheeledVehicle {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        let db = self.vehicle.datablock();
        opt(c, "wheeled.wheels?", &mut self.setup, |c, list| {
            let n = wheels(c, db, list.len())?;
            list.resize_with(n, Default::default);
            for w in list.iter_mut() {
                opt(c, "wheeled.wheel?", w, |c, w| {
                    datablock_id(c, "wheeled.tire", &mut w.tire)?;
                    datablock_id(c, "wheeled.spring", &mut w.spring)?;
                    c.flag("wheeled.powered", &mut w.powered)?;
                    c.raw32("wheeled.steering", &mut w.steering.0)
                })?;
            }
            Ok(())
        })?;
        c.flag("wheeled.controlled", &mut self.controlled)?;
        if self.controlled {
            self.motion = None;
            return Ok(());
        }
        let m = self.motion.get_or_insert_with(WheeledMotion::default);
        c.flag("wheeled.braking", &mut m.braking)?;
        opt(c, "wheeled.position?", &mut m.wheels, |c, list| {
            let n = wheels(c, db, list.len())?;
            list.resize_with(n, Default::default);
            for w in list.iter_mut() {
                c.floats("wheeled.spin", w)?;
            }
            Ok(())
        })
    }
}

pub fn pack_wheeled_vehicle(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (WheeledVehicle, u32) {
    let (vehicle, ret) = pack_vehicle_part(view, mask, ctx);
    let count = (int(view, "wheel_count", 0).max(0) as usize).min(WIRE_WHEELS);
    let setup = (mask & self::mask::WHEEL != 0).then(|| {
        (0..count)
            .map(|i| {
                let tire = id(view, "wheel_tire", i)?;
                let spring = id(view, "wheel_spring", i)?;
                Some(WheelSetup {
                    tire: ctx.wire_db(Some(tire)),
                    spring: ctx.wire_db(Some(spring)),
                    powered: boolean(view, "wheel_powered", i),
                    steering: raw(view, "wheel_steering", i),
                })
            })
            .collect()
    });
    let controlled = controlled(view, mask);
    let motion = (!controlled).then(|| WheeledMotion {
        braking: boolean(view, "braking", 0),
        wheels: (mask & self::mask::POSITION != 0).then(|| {
            (0..count)
                .map(|i| {
                    [
                        raw(view, "wheel_avel", i),
                        raw(view, "wheel_dy", i),
                        raw(view, "wheel_dx", i),
                    ]
                })
                .collect()
        }),
    });
    (
        WheeledVehicle {
            vehicle,
            setup,
            controlled,
            motion,
        },
        ret,
    )
}

/// FlyingVehicle's two fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlyingTail {
    pub create_height_on: bool,
    pub thrust: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlyingVehicle {
    pub vehicle: VehiclePart,
    /// Absent when the vehicle is controlled by this connection (the reader: when it is the client's control object).
    pub tail: Option<FlyingTail>,
}

impl Layout for FlyingVehicle {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        let present = match c.controlled_here() {
            Some(here) => !here,
            None => self.tail.is_some(),
        };
        if !present {
            self.tail = None;
            return Ok(());
        }
        let t = self.tail.get_or_insert_with(FlyingTail::default);
        c.flag("flying.createHeightOn", &mut t.create_height_on)?;
        c.int("flying.thrust", &mut t.thrust, THRUST_BITS)
    }
}

pub fn pack_flying_vehicle(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (FlyingVehicle, u32) {
    let (vehicle, ret) = pack_vehicle_part(view, mask, ctx);
    let tail = (!controlled(view, mask)).then(|| FlyingTail {
        create_height_on: boolean(view, "create_height_on", 0),
        thrust: mask_bits(int(view, "thrust_direction", 0) as u32, THRUST_BITS),
    });
    (FlyingVehicle { vehicle, tail }, ret)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoverVehicle {
    pub vehicle: VehiclePart,
    pub thrust: u32,
}

impl Layout for HoverVehicle {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        c.int("hover.thrust", &mut self.thrust, THRUST_BITS)
    }
}

pub fn pack_hover_vehicle(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (HoverVehicle, u32) {
    let (vehicle, ret) = pack_vehicle_part(view, mask, ctx);
    (
        HoverVehicle {
            vehicle,
            thrust: mask_bits(int(view, "thrust_direction", 0) as u32, THRUST_BITS),
        },
        ret,
    )
}
