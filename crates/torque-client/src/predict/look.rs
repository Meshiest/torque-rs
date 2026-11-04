//! The vehicles as the client draws them each frame, between its ticks.
//!
//! - **The wheels** (`WheeledVehicle::advanceTime`, each frame): every wheel with a tire and a spring turns by
//!   `avel * dt / 2pi` (its `apos`, kept in [0, 1)); `renderImage` draws it spun by `apos * 2pi` about its axle, let down
//!   its spring by `length * extension` and turned by `steering.x * wheel.steering`. The steering, the spin rate, the extension and
//!   the contact are the client's own vehicle's ([`vehicle_look`]): the server's updates carry no steering for the vehicle's
//!   controlling client (`Vehicle::packUpdate` writes the flag and returns), so the drawn steering of a driven vehicle was the
//!   record's first, 0 bits: full left.
//! - **A FlyingVehicle ghost** ([`FlyingWarp`]): TGE's client runs every vehicle ghost (`Vehicle::unpackUpdate`'s warp, then its
//!   physics); the engine runs the WheeledVehicles ([`super::drive::GhostVehicle`]) but not the FlyingVehicles. Engine approximation:
//!   from each update the drawn pose warps from where it was drawn to the update's over the ticks TGE's warp takes (the offset's
//!   length over the update's speed for a tick, 1 to 3 ticks), then goes on at the update's velocity for at most
//!   [`super::drive::MAX_PREDICTION_TICKS`] ticks, the rotation held.
//!
//! Caps: [`MAX_FLYING`] FlyingVehicle ghosts warped (others are drawn where their ghosts were sent), [`LOOK_WHEELS`] wheels a vehicle.
//!
//! Credit: follows TGE's vehicles as Torque3D carries them (MIT: `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp`,
//! each Copyright (c) 2012 GarageGames, LLC).

use std::collections::BTreeMap;

use ghost_world::world::{LOOK_WHEELS, Pose, VehicleLook, WheelLook};
use ghosts::Value;

use super::game::PredictWorld;
use crate::records::{EngineDatablock, EngineRecord};

use super::drive::{self, DrivenVehicle};

/// The most FlyingVehicle ghosts warped at once.
pub const MAX_FLYING: usize = 64;
/// A tick (ms).
const TICK_MS: f32 = 32.0;

/// A wheel's spin after `dt` seconds at `avel` (`advanceTime`: `apos = frac(avel * dt / 2pi + apos)`).
pub fn spin(apos: f32, avel: f32, dt: f32) -> f32 {
    let a = avel * dt / std::f32::consts::TAU + apos;
    let f = a - a.floor();
    if f.is_finite() && (0.0..1.0).contains(&f) {
        f
    } else {
        0.0
    }
}

/// The wheels' spins of the vehicles the client runs, advanced each frame by the frame's virtual time.
#[derive(Debug, Clone, Default)]
pub struct WheelSpins {
    pub spins: BTreeMap<u32, [f32; LOOK_WHEELS]>,
    pub last_ms: Option<u32>,
}

impl WheelSpins {
    /// Every vehicle of `vehicles` turned by the time since the last call (at most a second; a vehicle no longer run is
    /// forgotten).
    pub fn advance<'a>(&mut self, now_ms: u32, vehicles: impl Iterator<Item = &'a DrivenVehicle>) {
        let dt = self
            .last_ms
            .replace(now_ms)
            .map_or(0.0, |l| now_ms.wrapping_sub(l).min(1_000) as f32 / 1000.0);
        let mut kept = BTreeMap::new();
        for v in vehicles {
            let mut s = self
                .spins
                .get(&v.index)
                .copied()
                .unwrap_or([0.0; LOOK_WHEELS]);
            for (k, w) in v.v.k.wheels.iter().take(LOOK_WHEELS).enumerate() {
                if w.tire.is_some() && w.spring.is_some() {
                    s[k] = spin(s[k], w.avel, dt);
                }
            }
            kept.insert(v.index, s);
        }
        self.spins = kept;
    }
}

/// The look of a vehicle the client runs: its state after the last tick, its wheels' contact points moved to where the frame draws
/// it (`render`, the frame's position; the tick's own extension and contact, `extendWheels` run at the tick).
pub fn vehicle_look(
    v: &DrivenVehicle,
    spins: Option<&[f32; LOOK_WHEELS]>,
    render: [f32; 3],
) -> VehicleLook {
    let s = &v.v.s;
    let at = s.position();
    let shift = [0, 1, 2].map(|k| render[k] - at[k]);
    let wheels =
        v.v.k
            .wheels
            .iter()
            .take(LOOK_WHEELS)
            .enumerate()
            .map(|(k, w)| WheelLook {
                fitted: w.tire.is_some() && w.spring.is_some(),
                extension: w.extension,
                apos: spins.map_or(0.0, |sp| sp[k]),
                contact: w.contact,
                point: [0, 1, 2].map(|c| w.point[c] + shift[c]),
            })
            .collect();
    VehicleLook {
        steering: s.steering[0],
        velocity: s.rigid.lin_vel,
        wheels,
    }
}

/// A FlyingVehicle ghost's drawn pose between its updates (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct FlyingWarp {
    pub from: ([f32; 3], [f32; 4]),
    pub to: ([f32; 3], [f32; 4]),
    pub velocity: [f32; 3],
    /// The virtual time of the update (ms) and the warp's length (ms).
    pub at_ms: u32,
    pub warp_ms: f32,
}

impl FlyingWarp {
    /// The pose at `now_ms`.
    pub fn pose(&self, now_ms: u32) -> ([f32; 3], [f32; 4]) {
        let t = now_ms.wrapping_sub(self.at_ms) as f32;
        if t < self.warp_ms && self.warp_ms > 0.0 {
            let f = (t / self.warp_ms).clamp(0.0, 1.0);
            let p = [0, 1, 2].map(|k| self.from.0[k] + (self.to.0[k] - self.from.0[k]) * f);
            return (p, drive::quat_slerp(self.from.1, self.to.1, f));
        }
        let run = ((t - self.warp_ms) / 1000.0)
            .clamp(0.0, drive::MAX_PREDICTION_TICKS as f32 * TICK_MS / 1000.0);
        let p = [0, 1, 2].map(|k| self.to.0[k] + self.velocity[k] * run);
        (p, self.to.1)
    }
}

/// The FlyingVehicle ghosts' warps, from the world's vehicle updates in order.
#[derive(Debug, Clone, Default)]
pub struct FlyingWarps {
    pub warps: BTreeMap<u32, FlyingWarp>,
    pub seq: u64,
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl FlyingWarps {
    /// The updates since the last call taken at `now_ms` (each warps from the pose drawn then), the ghosts gone forgotten.
    pub fn update<W: PredictWorld>(&mut self, world: &W, now_ms: u32) {
        self.warps.retain(|i, _| world.ghosts.contains_key(i));
        let new: Vec<_> = world
            .vehicle_updates
            .iter()
            .filter(|u| u.seq > self.seq)
            .collect();
        for u in new {
            self.seq = u.seq;
            let Some(rec) = u.record.vehicle().filter(|v| v.flying) else {
                continue;
            };
            let (Some(pose), Some(position)) = (&rec.pose, u.position) else {
                continue;
            };
            let rotation = pose.rotation;
            if !finite(&position) || !finite(&rotation) {
                continue;
            }
            let mass = world
                .ghosts
                .get(&u.index)
                .and_then(|g| g.datablock())
                .and_then(|db| match world.datablocks.get(&db)?.value("mass", 0)? {
                    Value::F32(m) if *m > 0.0 && m.is_finite() => Some(*m),
                    _ => None,
                })
                .unwrap_or(1.0);
            let velocity = pose.linear.map(|x| x / mass);
            let velocity = if finite(&velocity) {
                velocity
            } else {
                [0.0; 3]
            };
            let from = match self.warps.get(&u.index) {
                Some(w) => w.pose(now_ms),
                None => (position, rotation),
            };
            if !self.warps.contains_key(&u.index) && self.warps.len() >= MAX_FLYING {
                continue;
            }
            // `Vehicle::unpackUpdate`'s warp length: the offset over the speed a tick covers, in whole ticks, 1 to 3 (none moving: 3).
            let off = [0, 1, 2].map(|k| position[k] - from.0[k]);
            let dist = (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt();
            let speed =
                (velocity[0] * velocity[0] + velocity[1] * velocity[1] + velocity[2] * velocity[2])
                    .sqrt()
                    * (TICK_MS / 1000.0);
            let ticks = if speed > 0.0 {
                (dist / speed).ceil().clamp(1.0, 3.0)
            } else {
                3.0
            };
            let warp_ms = if dist == 0.0 { 0.0 } else { ticks * TICK_MS };
            self.warps.insert(
                u.index,
                FlyingWarp {
                    from,
                    to: (position, rotation),
                    velocity,
                    at_ms: now_ms,
                    warp_ms,
                },
            );
        }
    }

    /// The drawn poses at `now_ms`.
    pub fn poses(&self, now_ms: u32) -> impl Iterator<Item = (u32, Pose)> + '_ {
        self.warps.iter().map(move |(i, w)| {
            let (position, rotation) = w.pose(now_ms);
            (*i, Pose::Quat { position, rotation })
        })
    }
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod tests;
