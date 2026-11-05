//! The vehicles as the client draws them each frame, between its ticks.
//!
//! - **The wheels** (`WheeledVehicle::advanceTime`, each frame): every wheel with a tire and a spring turns by
//!   `avel * dt / 2pi` (its `apos`, kept in [0, 1)); `renderImage` draws it spun by `apos * 2pi` about its axle, let down
//!   its spring by `length * extension` and turned by `steering.x * wheel.steering`. The steering, the spin rate, the extension and
//!   the contact are the client's own vehicle's ([`vehicle_look`]): the server's updates carry no steering for the vehicle's
//!   controlling client (`Vehicle::packUpdate` writes the flag and returns), so the drawn steering of a driven vehicle was the
//!   record's first, 0 bits: full left. Engine addition: the drawn steering eases each frame toward the steering the ticks
//!   set ([`SteeringEase`]: a critically damped follow, frame-rate independent; another's vehicle's toward a glide between
//!   the values its updates bring), so the wheels, a `steering` thread and the hands on it turn smoothly between ticks and
//!   between updates instead of in their steps (TGE draws `mSteering` as the last tick left it). Each wheel's contact is drawn under the drawn tire: the hub where the frame draws the
//!   vehicle, down the spring by its extension and the tire's radius (`extendWheels` cast its ray from the pose before the
//!   tick's move, a tick behind the drawn vehicle at speed).
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

use collide::math::{mul_p, quat_matrix};
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

/// How closely a drawn steering follows its target (seconds): the time constant of [`SteeringEase`]'s critically damped
/// follow. A step settles in about four of them; a steady turn is drawn twice this behind.
pub const STEER_EASE_SECONDS: f32 = 0.03;
/// The shortest and the longest a networked steering takes to glide to a new value (ms): a tick, and a quarter of a second.
pub const STEER_GLIDE_MS: (u32, u32) = (32, 250);

/// A drawn steering and how fast it moves (radians, radians a second).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Eased {
    value: f32,
    rate: f32,
}

impl Eased {
    /// `dt` seconds of a critically damped follow of `target` (held over the step) with time constant `tau`: the closed form,
    /// so the same time drawn in many frames or in one ends at the same place.
    fn follow(self, target: f32, tau: f32, dt: f32) -> Eased {
        let w = 1.0 / tau;
        let c1 = self.value - target;
        let c2 = self.rate + w * c1;
        let e = (-w * dt).exp();
        Eased {
            value: target + (c1 + c2 * dt) * e,
            rate: (self.rate - w * c2 * dt) * e,
        }
    }
}

/// A networked steering's target between its values: from where it was when a new value came to that value, in a straight
/// line over the time the one before took to come ([`STEER_GLIDE_MS`] at most and at least), then held.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    from: f32,
    to: f32,
    start_ms: u32,
    span_ms: u32,
}

impl Glide {
    fn at_rest(value: f32, now_ms: u32) -> Glide {
        Glide {
            from: value,
            to: value,
            start_ms: now_ms,
            span_ms: STEER_GLIDE_MS.1,
        }
    }

    /// The target at `now_ms`.
    fn at(&self, now_ms: u32) -> f32 {
        let t = now_ms.wrapping_sub(self.start_ms) as f32 / self.span_ms.max(1) as f32;
        self.from + (self.to - self.from) * t.clamp(0.0, 1.0)
    }

    /// `value` taken at `now_ms`: a new value starts a new glide from where this one is, over the time since the last began.
    fn take(self, value: f32, now_ms: u32) -> Glide {
        if value == self.to {
            return self;
        }
        Glide {
            from: self.at(now_ms),
            to: value,
            start_ms: now_ms,
            span_ms: now_ms
                .wrapping_sub(self.start_ms)
                .clamp(STEER_GLIDE_MS.0, STEER_GLIDE_MS.1),
        }
    }
}

/// One vehicle's drawn steering, and the glide of a networked one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drawn {
    eased: Eased,
    glide: Option<Glide>,
}

/// The steering the frames draw for each vehicle the client runs (see the module docs), advanced each frame by the frame's
/// virtual time: the client's own vehicle's follows its ticks' steering; another's follows a glide between the values its
/// updates bring ([`Glide`]), so its wheel turns evenly between them. `seconds` is the follow's time constant; `None` draws
/// the last tick's steering, as TGE does.
#[derive(Debug, Clone)]
pub struct SteeringEase {
    pub seconds: Option<f32>,
    drawn: BTreeMap<u32, Drawn>,
    last_ms: Option<u32>,
}

impl Default for SteeringEase {
    fn default() -> SteeringEase {
        SteeringEase {
            seconds: Some(STEER_EASE_SECONDS),
            drawn: BTreeMap::new(),
            last_ms: None,
        }
    }
}

impl SteeringEase {
    /// Every vehicle of `vehicles` (each with whether its steering comes from the network: another's vehicle) followed for the
    /// time since the last call (at most a second); a vehicle first seen starts at its steering, one no longer run is
    /// forgotten. The drawn steering stays within the vehicle's lock.
    pub fn advance<'a>(
        &mut self,
        now_ms: u32,
        vehicles: impl Iterator<Item = (&'a DrivenVehicle, bool)>,
    ) {
        let dt = self
            .last_ms
            .replace(now_ms)
            .map_or(0.0, |l| now_ms.wrapping_sub(l).min(1_000) as f32 / 1000.0);
        let mut kept = BTreeMap::new();
        for (v, networked) in vehicles {
            let steering = v.v.s.steering[0];
            if !steering.is_finite() {
                continue;
            }
            let lock = v.data.base.max_steering_angle.abs();
            let last = self
                .drawn
                .get(&v.index)
                .filter(|d| d.eased.value.is_finite());
            let glide = networked.then(|| match last.and_then(|d| d.glide) {
                Some(g) => g.take(steering, now_ms),
                None => Glide::at_rest(steering, now_ms),
            });
            let target = glide.map_or(steering, |g| g.at(now_ms));
            let mut eased = match (last, self.seconds) {
                (Some(d), Some(tau)) if tau > 0.0 => d.eased.follow(target, tau, dt),
                _ => Eased {
                    value: steering,
                    rate: 0.0,
                },
            };
            if lock.is_finite() && eased.value.abs() > lock {
                eased = Eased {
                    value: eased.value.clamp(-lock, lock),
                    rate: 0.0,
                };
            }
            kept.insert(v.index, Drawn { eased, glide });
        }
        self.drawn = kept;
    }

    /// The steering drawn for vehicle `index` (none before its first frame).
    pub fn steering(&self, index: u32) -> Option<f32> {
        self.drawn.get(&index).map(|d| d.eased.value)
    }
}

/// The look of a vehicle the client runs: its state after the last tick, drawn with `steering` (its own when none), each
/// wheel's contact under its tire where the frame draws the vehicle (`render`: the hub down the spring by the tick's extension
/// and the tire's radius; the tick's own contact, `extendWheels` run at the tick).
pub fn vehicle_look(
    v: &DrivenVehicle,
    spins: Option<&[f32; LOOK_WHEELS]>,
    steering: Option<f32>,
    render: &Pose,
) -> VehicleLook {
    let s = &v.v.s;
    let m = match *render {
        Pose::Quat { position, rotation } => {
            let mut m = quat_matrix(rotation);
            m[3] = position[0];
            m[7] = position[1];
            m[11] = position[2];
            m
        }
        _ => s.render_to_world,
    };
    let wheels =
        v.v.k
            .wheels
            .iter()
            .take(LOOK_WHEELS)
            .enumerate()
            .map(|(k, w)| {
                let under = match (&w.tire, &w.spring, v.data.wheels.get(k)) {
                    (Some((_, tire)), Some((_, spring)), Some(rec)) => {
                        let ext = if w.extension.is_finite() {
                            w.extension.clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let drop = spring.length * ext + tire.radius;
                        Some(mul_p(&m, [rec.pos[0], rec.pos[1], rec.pos[2] - drop]))
                    }
                    _ => None,
                };
                WheelLook {
                    fitted: w.tire.is_some() && w.spring.is_some(),
                    extension: w.extension,
                    apos: spins.map_or(0.0, |sp| sp[k]),
                    contact: w.contact,
                    point: under
                        .filter(|p| p.iter().all(|c| c.is_finite()))
                        .unwrap_or(w.point),
                    slip: w.slip,
                }
            })
            .collect();
    VehicleLook {
        steering: steering.unwrap_or(s.steering[0]),
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
