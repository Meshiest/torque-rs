//! TGE's records as the client's engine half reads them ([`super`]): the classes the prediction sorts, a Player's and a vehicle's
//! packet data (torque-ghosts' [`TgePacketData`]), the ShapeBase, Player and vehicle blocks of a [`TgeRecord`], the datablocks'
//! fields ([`TgeDatablock`]) and the control section's tails ([`TgeTail`]), over [`TgeClientSet`].
//!
//! TGE's Player packet data carries a recharge rate, a state, the recover ticks and the jump delay beside the words the engine
//! half keeps ([`PlayerWords`]); packet data made from the words ([`EngineSet::player_data`]) sends none of them (0, the move state,
//! no recover, no delay). TGE's has no word beside its own (`flag` reads false). A HoverVehicle's packet data reads as a
//! FlyingVehicle's (no wheels).
//!
//! Credit: the layouts read here are TGE 1.4's `writePacketData` / `packUpdate` as torque-ghosts carries them (Torque3D, MIT:
//! `T3D/shapeBase.cpp`, `T3D/player.cpp`, each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.;
//! `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp`, each Copyright (c) 2012 GarageGames, LLC).

use collide::math::F3;
use ghost_world::tge::{TgeClientSet, TgeDatablock, TgeTail, shape_part};
use ghosts::codec::F32;
use ghosts::tails::{
    PlayerTail, PlayerTailMotion, ShapeTail, TailData, VehicleTail, WheelTail, WheeledTail,
};
use ghosts::vehicle::VehiclePart;
use ghosts::{TgeClass, TgePacketData, TgeRecord, TgeShapeBase, Value};
use net::control::ControlData;
use vehicle::words::{KindWords, VehicleWords, WheelWords};

use super::{
    ClassKind, EngineDatablock, EngineRecord, EngineSet, EngineTail, FreeMotion, PlayerPacket,
    PlayerWords, ShapeState, ShotRecord, VehiclePose, VehicleUpdate, WheelDatablocks,
};

fn player_words(p: &PlayerTail, translation: [f32; 3]) -> PlayerWords {
    PlayerWords {
        energy: p.shape.energy.get(),
        flag: false,
        free: p.motion.map(|m| FreeMotion {
            position: m.position.map(F32::get),
            velocity: m.velocity.map(F32::get),
            contact: m.last_contact,
        }),
        head_x: p.head[0].get(),
        head_z: p.head[1].get(),
        rot_z: p.rotation_z.get(),
        translation,
    }
}

fn packet(data: &dyn ControlData) -> Option<&TgePacketData> {
    (data as &dyn std::any::Any).downcast_ref::<TgePacketData>()
}

fn vehicle_words(v: &VehicleTail, kind: KindWords) -> VehicleWords {
    let f3 = |v: [F32; 3]| v.map(F32::get);
    VehicleWords {
        energy: v.shape.energy.get(),
        steering: v.steering.map(F32::get),
        position: f3(v.position),
        rotation: v.rotation.map(F32::get),
        lin_mom: f3(v.linear_momentum),
        ang_mom: f3(v.angular_momentum),
        at_rest: v.at_rest,
        contacts_zero: v.no_contacts,
        disable_move: v.disable_move,
        kind,
    }
}

impl EngineSet for TgeClientSet {
    fn class_kind(class: TgeClass) -> ClassKind {
        match class {
            TgeClass::Player => ClassKind::Player,
            TgeClass::AiPlayer => ClassKind::AiPlayer,
            TgeClass::WheeledVehicle | TgeClass::AiWheeledVehicle => ClassKind::WheeledVehicle,
            TgeClass::FlyingVehicle => ClassKind::FlyingVehicle,
            TgeClass::StaticShape => ClassKind::StaticShape,
            _ => ClassKind::Other,
        }
    }

    fn read_player(data: &dyn ControlData) -> Option<PlayerPacket<'_>> {
        let p = packet(data)?;
        let TailData::Player(t) = &p.data else {
            return None;
        };
        Some(PlayerPacket {
            words: player_words(t, p.position),
            controlled: t.control.zip(p.nested.as_deref()),
        })
    }

    /// `Player::writePacketData`: the words, the last contact cut to 15 as the writer cuts it.
    fn player_data(
        words: &PlayerWords,
        controlled: Option<(u32, Box<dyn ControlData>)>,
    ) -> Box<dyn ControlData> {
        let (control, nested) = controlled.map_or((None, None), |(i, d)| (Some(i), Some(d)));
        let tail = PlayerTail {
            shape: ShapeTail {
                energy: F32::new(words.energy),
                recharge_rate: F32::new(0.0),
            },
            motion: words.free.map(|f| PlayerTailMotion {
                position: f.position.map(F32::new),
                velocity: f.velocity.map(F32::new),
                last_contact: f.contact.min(15),
            }),
            head: [F32::new(words.head_x), F32::new(words.head_z)],
            rotation_z: F32::new(words.rot_z),
            control,
            ..PlayerTail::default()
        };
        Box::new(TgePacketData {
            data: TailData::Player(tail),
            position: words.translation,
            nested,
        })
    }

    fn read_vehicle(data: &dyn ControlData) -> Option<VehicleWords> {
        Some(match &packet(data)?.data {
            TailData::Vehicle(v) => vehicle_words(v, KindWords::Flying),
            TailData::Wheeled(w) => vehicle_words(
                &w.vehicle,
                KindWords::Wheeled {
                    brake: w.braking,
                    wheels: w
                        .wheels
                        .iter()
                        .map(|x| WheelWords {
                            avel: x.avel.get(),
                            dy: x.dy.get(),
                            dx: x.dx.get(),
                            slipping: x.slipping,
                        })
                        .collect(),
                },
            ),
            _ => return None,
        })
    }

    /// The vehicle's `writePacketData` (a WheeledVehicle's with its wheels), its recharge rate 0.
    fn vehicle_data(w: &VehicleWords, translation: F3) -> Box<dyn ControlData> {
        let f3 = |v: F3| v.map(F32::new);
        let vehicle = VehicleTail {
            shape: ShapeTail {
                energy: F32::new(w.energy),
                recharge_rate: F32::new(0.0),
            },
            steering: w.steering.map(F32::new),
            position: f3(w.position),
            rotation: w.rotation.map(F32::new),
            linear_momentum: f3(w.lin_mom),
            angular_momentum: f3(w.ang_mom),
            at_rest: w.at_rest,
            no_contacts: w.contacts_zero,
            disable_move: w.disable_move,
        };
        let data = match &w.kind {
            KindWords::Flying => TailData::Vehicle(vehicle),
            KindWords::Wheeled { brake, wheels } => TailData::Wheeled(WheeledTail {
                vehicle,
                braking: *brake,
                wheels: wheels
                    .iter()
                    .map(|x| WheelTail {
                        avel: F32::new(x.avel),
                        dy: F32::new(x.dy),
                        dx: F32::new(x.dx),
                        slipping: x.slipping,
                    })
                    .collect(),
            }),
        };
        Box::new(TgePacketData {
            data,
            position: translation,
            nested: None,
        })
    }
}

fn shape_state(shape: &TgeShapeBase) -> ShapeState {
    let body = shape.body.as_ref();
    ShapeState {
        damage: body.and_then(|b| b.damage.as_ref()).map(|d| d.state),
        mount: body
            .and_then(|b| b.mount)
            .flatten()
            .map(|m| (m.object, m.node)),
        controlled: body
            .and_then(|b| b.rare.as_ref()?.cloak.as_ref())
            .is_some_and(|c| c.controlled),
    }
}

fn vehicle_update(part: &VehiclePart, flying: bool) -> VehicleUpdate {
    let state = part.state.as_ref();
    VehicleUpdate {
        flying,
        controlled: part.controlled,
        steering: state.map_or([0; 2], |s| s.steering),
        last_move: state.map(|s| s.mv.to_move()).unwrap_or_default(),
        pose: state.and_then(|s| s.rigid.as_ref()).map(|r| VehiclePose {
            rotation: r.rotation.map(F32::get),
            linear: r.linear_momentum.map(F32::get),
            angular: r.angular_momentum.map(F32::get),
            at_rest: r.at_rest,
            warp: false,
        }),
        brake: false,
        wheels: None,
        wheel_datablocks: None,
    }
}

impl EngineRecord<TgeClientSet> for TgeRecord {
    fn shape_state(&self) -> Option<ShapeState> {
        shape_part(self).map(shape_state)
    }

    fn player_state(&self) -> Option<ShapeState> {
        match self {
            TgeRecord::Player(p) | TgeRecord::AiPlayer(p) => Some(shape_state(&p.shape)),
            _ => None,
        }
    }

    fn vehicle(&self) -> Option<VehicleUpdate> {
        match self {
            TgeRecord::WheeledVehicle(w) | TgeRecord::AiWheeledVehicle(w) => Some(VehicleUpdate {
                brake: w.motion.as_ref().is_some_and(|m| m.braking),
                wheels: w.motion.as_ref().and_then(|m| m.wheels.as_ref()).map(|v| {
                    v.iter()
                        .map(|[avel, dy, dx]| WheelWords {
                            avel: avel.get(),
                            dy: dy.get(),
                            dx: dx.get(),
                            slipping: false,
                        })
                        .collect()
                }),
                wheel_datablocks: w.setup.as_ref().map(|v| {
                    v.iter()
                        .map(|s| {
                            s.as_ref().map(|s| WheelDatablocks {
                                tire: s.tire,
                                spring: s.spring,
                            })
                        })
                        .collect()
                }),
                ..vehicle_update(&w.vehicle, false)
            }),
            TgeRecord::FlyingVehicle(f) => Some(vehicle_update(&f.vehicle, true)),
            _ => None,
        }
    }

    fn player_motion(&self) -> Option<(F3, f32)> {
        let (TgeRecord::Player(p) | TgeRecord::AiPlayer(p)) = self else {
            return None;
        };
        let m = p.motion.as_ref()?;
        let velocity = m
            .velocity
            .map_or([0.0; 3], |v| v.value(ghosts::player::VELOCITY_NORMAL_BITS));
        let head = ghost_world::tge::head_fractions(self).map_or(0.0, |h| h[0]);
        Some((velocity, head))
    }

    fn shot(&self) -> Option<ShotRecord> {
        let TgeRecord::Projectile(p) = self else {
            return None;
        };
        let initial = p.initial.as_ref()?;
        // The bounce block's raw floats, when the record has it, over the initial block's coarser direction.
        let (start, velocity) = match p.bounce {
            Some((at, v)) => (Some(at.map(|c| c.get())), v.map(|c| c.get())),
            None => (
                None,
                initial.velocity.map_or([0.0; 3], |v| {
                    v.value(ghosts::projectile::VELOCITY_NORMAL_BITS)
                }),
            ),
        };
        Some(ShotRecord {
            start,
            velocity,
            tick: initial.tick,
            source: initial.source.map(|s| s.object),
            exploded: p.explosion.map(|x| x.position.map(|c| c.get())),
        })
    }
}

impl EngineDatablock for TgeDatablock {
    fn class_name(&self) -> &str {
        &self.class
    }

    fn value(&self, name: &str, index: u32) -> Option<&Value> {
        self.get(name, index)
    }
}

impl EngineTail for TgeTail {
    fn player(&self) -> Option<PlayerWords> {
        match self {
            TgeTail::Data(p) => match &p.data {
                TailData::Player(t) => Some(player_words(t, p.position)),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "tge_tests.rs"]
mod tests;
