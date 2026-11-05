//! The dust a vehicle kicks up, run by the client for the vehicles it runs (TGE's `WheeledVehicle::updateWheelParticles` and
//! `Vehicle::updateLiftoffDust`): particles of the datablock's emitters through the particle engine ([`crate::particles`]).
//!
//! - **The tires** (`WheeledVehicleData::tireEmitter`, an emitter of its own on each wheel): while the vehicle moves faster than
//!   [`MIN_SPEED`], each wheel with a tire and a spring that touches the ground emits at its contact point where the frame draws
//!   it (engine choice: TGE emits along the path from where the point was to where it is, which at speed starts the dust a
//!   frame's travel behind the tire; the particles inherit the vehicle's velocity as their datablock says, so they still trail
//!   off), the axis the vehicle's direction of travel and the velocity the vehicle's, for `dt * (speed /
//!   maxWheelSpeed) * slip` of its clock (the wheel's slip, `1 -` the share of its force the friction circle let through): a
//!   rolling wheel throws little, a spinning or sliding one a cloud. A game may lay a floor under the slip
//!   ([`DustRules::min_slip`], none in TGE) so a rolling wheel throws a little dust too. A wheel off the ground breaks its path.
//! - **Lift-off dust** (`VehicleData::dustEmitter`, `dustHeight`, `triggerDustHeight`): while the ground (the host's `ground`
//!   ray, [`VehicleDust::update`]) is within `triggerDustHeight` under the vehicle, its emitter emits at the ground point raised
//!   `dustHeight`, along the ground's normal, the vehicle's velocity inherited, for the frame's time.
//! - **Colors**: TGE takes a terrain material's puff colors; the engine's maps carry none, so the particles keep their
//!   datablock's.
//! - **Caps**: the emitters of the particle engine ([`MAX_DUST_EMITTERS`]); an emitter of a vehicle the client no longer runs is
//!   let go once its particles died.
//!
//! Credit: follows TGE 1.4's `game/vehicles/wheeledVehicle.cc` and `game/vehicles/vehicle.cc` (Copyright (C) GarageGames.com,
//! Inc.), as Torque3D carries them under the MIT license: `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/vehicle.cpp`
//! (each Copyright (c) 2012 GarageGames, LLC) and `ShapeBase::emitDust` in `T3D/shapeBase.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::collections::BTreeMap;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::{ClientWorld, VehicleLook};
use ghosts::Value;

use crate::Draw;
use crate::particles::{Emitter, Rng};
use crate::textures::TextureSet;

/// The speed a vehicle must pass for its tires to throw dust (units a second).
pub const MIN_SPEED: f32 = 1.0;
/// The most dust emitters kept (a wheel's, a vehicle's lift-off dust).
pub const MAX_DUST_EMITTERS: usize = 256;
/// The longest frame a dust emitter runs (seconds: a stalled frame does not pour out dust).
pub const MAX_FRAME: f32 = 0.25;

/// A game's choices about the dust beyond TGE's rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DustRules {
    /// The least slip a wheel in contact throws dust by (0: TGE's, a rolling wheel throws none).
    pub min_slip: f32,
}

impl Default for DustRules {
    fn default() -> DustRules {
        DustRules { min_slip: 0.0 }
    }
}

/// The host's ray to the ground: from a point straight down at most a distance, the hit point and the ground's normal.
pub type GroundRay<'a> = dyn Fn([f32; 3], f32) -> Option<([f32; 3], [f32; 3])> + 'a;

/// A vehicle the client runs, as the dust reads it: its ghost, its datablock, where the frame draws it and its look (velocity,
/// wheels).
#[derive(Debug, Clone, Copy)]
pub struct DustVehicle<'a> {
    pub index: u32,
    pub datablock: u32,
    pub position: [f32; 3],
    pub look: &'a VehicleLook,
}

/// The vehicles `world`'s client runs ([`ClientWorld::vehicle_look`]) with their datablocks and drawn places.
pub fn running_vehicles(world: &ClientWorld<TgeClientSet>) -> Vec<DustVehicle<'_>> {
    world
        .vehicle_look
        .iter()
        .filter_map(|(&index, look)| {
            let datablock = world.ghosts.get(&index)?.datablock()?;
            let position = world
                .vehicle_render
                .get(&index)
                .and_then(|p| crate::mounts::pose_matrix(p, None))
                .map(|(m, _)| m.column(3))?;
            Some(DustVehicle {
                index,
                datablock,
                position,
                look,
            })
        })
        .collect()
}

/// The ground straight under `at` within `depth`, among `triangles`: the highest point there and its upward normal (a
/// [`GroundRay`] over a host's surfaces).
pub fn ground_under(
    triangles: &[crate::decals::Triangle],
    at: [f32; 3],
    depth: f32,
) -> Option<([f32; 3], [f32; 3])> {
    let mut best: Option<([f32; 3], [f32; 3])> = None;
    for t in triangles {
        let e1 = [0, 1, 2].map(|k| t[1][k] - t[0][k]);
        let e2 = [0, 1, 2].map(|k| t[2][k] - t[0][k]);
        let mut n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        if n[2].abs() < 1e-6 {
            continue;
        }
        if n[2] < 0.0 {
            n = n.map(|c| -c);
        }
        // Inside the triangle seen from above (the same side of each edge).
        let side = |a: [f32; 3], b: [f32; 3]| {
            (b[0] - a[0]) * (at[1] - a[1]) - (b[1] - a[1]) * (at[0] - a[0])
        };
        let s = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
        if !(s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)) {
            continue;
        }
        let z = t[0][2] - (n[0] * (at[0] - t[0][0]) + n[1] * (at[1] - t[0][1])) / n[2];
        if z > at[2] || at[2] - z > depth || best.is_some_and(|b| b.0[2] >= z) {
            continue;
        }
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        best = Some(([at[0], at[1], z], n.map(|c| c / l)));
    }
    best
}

/// One emitter and the datablock it was made of.
#[derive(Debug, Clone)]
struct Kept {
    datablock: u32,
    emitter: Emitter,
    /// Emitted this frame (the others break their path).
    live: bool,
}

/// The dust of the vehicles the client runs (module docs).
#[derive(Debug, Clone)]
pub struct VehicleDust {
    pub rules: DustRules,
    /// By (ghost, wheel); the lift-off dust's wheel is `usize::MAX`.
    emitters: BTreeMap<(u32, usize), Kept>,
    rng: Rng,
    live: usize,
}

impl Default for VehicleDust {
    fn default() -> VehicleDust {
        VehicleDust::new(DustRules::default())
    }
}

/// The lift-off dust's place among a vehicle's emitters.
const LIFTOFF: usize = usize::MAX;

fn id_field(world: &ClientWorld<TgeClientSet>, db: u32, name: &str) -> Option<u32> {
    match world.datablocks.get(&db)?.get(name, 0)? {
        Value::Id(id) => *id,
        _ => None,
    }
}

fn float_field(world: &ClientWorld<TgeClientSet>, db: u32, name: &str) -> Option<f32> {
    match world.datablocks.get(&db)?.get(name, 0)? {
        Value::F32(x) if x.is_finite() => Some(*x),
        _ => None,
    }
}

impl VehicleDust {
    pub fn new(rules: DustRules) -> VehicleDust {
        VehicleDust {
            rules,
            emitters: BTreeMap::new(),
            rng: Rng::new(0x5eed_d057),
            live: 0,
        }
    }

    /// Forgets every emitter (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.emitters.clear();
        self.live = 0;
    }

    /// The particles alive, every emitter's: each with its vehicle's ghost and whether its emitter is a tire's (else the
    /// lift-off dust's).
    pub fn alive(&self) -> impl Iterator<Item = (u32, bool, &crate::particles::Particle)> {
        self.emitters.iter().flat_map(|((g, w), k)| {
            k.emitter
                .particles
                .iter()
                .map(move |p| (*g, *w != LIFTOFF, p))
        })
    }

    /// The particles alive.
    pub fn particles(&self) -> usize {
        self.emitters
            .values()
            .map(|k| k.emitter.particles.len())
            .sum()
    }

    /// The emitter of `key` of datablock `datablock` in `emitters`, made when it is new (or its datablock changed).
    fn emitter<'e>(
        emitters: &'e mut BTreeMap<(u32, usize), Kept>,
        world: &ClientWorld<TgeClientSet>,
        key: (u32, usize),
        datablock: u32,
    ) -> Option<&'e mut Kept> {
        let fresh = emitters.get(&key).is_none_or(|k| k.datablock != datablock);
        if fresh {
            if emitters.len() >= MAX_DUST_EMITTERS && !emitters.contains_key(&key) {
                return None;
            }
            let emitter = Emitter::new(world, datablock)?;
            emitters.insert(
                key,
                Kept {
                    datablock,
                    emitter,
                    live: false,
                },
            );
        }
        emitters.get_mut(&key)
    }

    /// The frame's dust, `dt` seconds after the last: the particles stepped, then each of `vehicles` ([`running_vehicles`])
    /// emitting as its wheels and its height over `ground` say (module docs), its emitters made of `world`'s datablocks.
    pub fn update(
        &mut self,
        world: &ClientWorld<TgeClientSet>,
        vehicles: &[DustVehicle<'_>],
        ground: &GroundRay<'_>,
        dt: f32,
    ) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_FRAME)
        } else {
            0.0
        };
        let ms = (dt * 1000.0) as u32;
        let mut live = self.live;
        for k in self.emitters.values_mut() {
            k.emitter.advance(ms, &mut live);
            k.live = false;
        }
        self.live = live;
        for v in vehicles {
            let (index, db, look) = (v.index, v.datablock, v.look);
            let vel = look.velocity;
            let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
            if let Some(tire) = id_field(world, db, "tireEmitter")
                && speed > MIN_SPEED
            {
                let axis = vel.map(|v| v / speed);
                let top = float_field(world, db, "maxWheelSpeed")
                    .unwrap_or(30.0)
                    .max(0.1);
                let min_slip = self.rules.min_slip;
                for (w, wheel) in look.wheels.iter().enumerate() {
                    if !(wheel.fitted && wheel.contact) {
                        continue;
                    }
                    let slip = wheel.slip.max(min_slip).clamp(0.0, 1.0);
                    let wheel_ms = (dt * (speed / top) * 1000.0 * slip) as u32;
                    if let Some(k) = Self::emitter(&mut self.emitters, world, (index, w), tire) {
                        k.emitter.break_path();
                        k.emitter.emit_path(
                            wheel.point,
                            axis,
                            vel,
                            wheel_ms,
                            &mut self.rng,
                            &mut self.live,
                        );
                        k.live = true;
                    }
                }
            }
            if let Some(dust) = id_field(world, db, "dustEmitter") {
                let trigger = float_field(world, db, "triggerDustHeight").unwrap_or(3.0);
                let height = float_field(world, db, "dustHeight").unwrap_or(1.0);
                if let Some((point, normal)) = ground(v.position, trigger) {
                    let from = [point[0], point[1], point[2] + height];
                    if let Some(k) =
                        Self::emitter(&mut self.emitters, world, (index, LIFTOFF), dust)
                    {
                        k.emitter
                            .emit_path(from, normal, vel, ms, &mut self.rng, &mut self.live);
                        k.live = true;
                    }
                }
            }
        }
        // A wheel that did not emit starts a new path when it touches again; an emitter of a vehicle gone goes once empty.
        let running: Vec<u32> = vehicles.iter().map(|v| v.index).collect();
        let mut gone = 0;
        self.emitters.retain(|(index, _), k| {
            if !k.live {
                k.emitter.break_path();
            }
            let keep = running.contains(index) || !k.emitter.particles.is_empty();
            if !keep {
                gone += k.emitter.particles.len();
            }
            keep
        });
        self.live = self.live.saturating_sub(gone);
    }

    /// The dust as draws, each emitter at its center's distance from `eye`, billboards along the camera's `right` and `up`.
    pub fn draws(
        &self,
        eye: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        textures: &mut TextureSet<'_>,
    ) -> Vec<(f32, Draw)> {
        self.emitters
            .values()
            .filter_map(|k| {
                let c = k.emitter.centre()?;
                let d = k.emitter.draw(right, up, textures, "dust")?;
                let dist = (0..3).map(|i| (c[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
                Some((dist, d))
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "vehicle_dust_tests.rs"]
mod tests;
