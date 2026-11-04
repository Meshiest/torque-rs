//! The objects' tickers: the adapters that implement [`::sim::Ticking`] over the host's context ([`TickCx`]).
//!
//! - `Camera` (Torque: `Camera::processTick`): [`CameraTicker`]: the camera class's tick
//!   ([`crate::scene::camera::CameraClass`]), with or without a move
//! - `Player`, `AIPlayer` (Torque: `Player::processTick`): [`super::player::tick::PlayerTicker`]
//! - `Item` (Torque: `Item::processTick`): [`super::item::ItemTicker`]
//! - `Projectile` (Torque: `Projectile::processTick`): the host's ([`GameTickers`])
//! - `WheeledVehicle`, `FlyingVehicle` (Torque: `Vehicle::processTick`): [`super::vehicle_tick::VehicleTicker`]
//! - every other `GameBase` class (Torque: its own): [`NoopTicker`] (not simulated here)
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/gameBase/processList.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is
//! this crate's own.

use ::sim::{Move, Ticking};
use net::moves::Move as NetMove;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use super::{TickCx, Ticker};
use crate::scene::camera::{camera_class, is_camera};

/// The `sim` form of a wire move: the floats and the triggers `processTick` reads.
pub fn sim_move(m: &NetMove) -> Move {
    Move {
        x: m.x,
        y: m.y,
        z: m.z,
        yaw: m.yaw,
        pitch: m.pitch,
        roll: m.roll,
        free_look: m.free_look,
        triggers: std::array::from_fn(|i| m.triggers[i]),
    }
}

/// The wire form back, for code that still takes one (`camera_tick` reads the floats and the triggers only; the raw fields stay
/// 0).
pub fn net_move(m: &Move) -> NetMove {
    NetMove {
        x: m.x,
        y: m.y,
        z: m.z,
        yaw: m.yaw,
        pitch: m.pitch,
        roll: m.roll,
        free_look: m.free_look,
        triggers: std::array::from_fn(|i| m.triggers.get(i).copied().unwrap_or(false)),
        ..NetMove::default()
    }
}

/// A `Camera`'s tick.
pub struct CameraTicker {
    pub id: ObjId,
}

impl Ticking<TickCx<'_>> for CameraTicker {
    fn class(&self) -> &'static str {
        "Camera"
    }

    fn process_tick(&mut self, cx: &mut TickCx<'_>, mv: Option<&Move>) {
        // A script that fails in the tick has printed its error; the tick is over.
        let class = camera_class(cx.vm);
        let _ = match mv {
            Some(m) => class.process_tick(cx.vm, self.id, &net_move(m), &mut *cx.calls),
            None => class.process_tick_idle(cx.vm, self.id),
        };
    }
}

/// The tick of a class with no tick of its own: nothing happens.
pub struct NoopTicker {
    class: &'static str,
}

impl Ticking<TickCx<'_>> for NoopTicker {
    fn class(&self) -> &'static str {
        self.class
    }

    fn process_tick(&mut self, _cx: &mut TickCx<'_>, _mv: Option<&Move>) {}
}

/// The classes named here; any other is a plain `GameBase`.
const KNOWN: &[&str] = &[
    "StaticShape",
    "Player",
    "AIPlayer",
    "Item",
    "Projectile",
    "WheeledVehicle",
    "FlyingVehicle",
    "fxLight",
    "ParticleEmitterNode",
    "SpawnSphere",
    "MissionMarker",
    "WayPoint",
    "Trigger",
    "PathCamera",
];

/// The host's tickers for classes it simulates itself (a `Vm` extension, [`set_game_tickers`]): asked first, by class name.
pub struct GameTickers(
    pub fn(&'static str, ObjId, &std::rc::Rc<super::player::tick::SimEnv>) -> Option<Ticker>,
);

/// The host's tickers for `vm` from now on.
pub fn set_game_tickers(vm: &mut Vm, tickers: GameTickers) {
    vm.set_extension(std::rc::Rc::new(tickers));
}

/// The ticker for the class of object `id` (`env`: what a Player's tick needs; without it a Player gets no tick).
pub fn ticker_for(
    vm: &Vm,
    id: ObjId,
    env: Option<std::rc::Rc<super::player::tick::SimEnv>>,
) -> Ticker {
    if is_camera(vm, id) {
        return Box::new(CameraTicker { id });
    }
    let objects = vm.objects();
    let name = objects
        .get(id)
        .and_then(|o| objects.classes.get(o.class()))
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let class = KNOWN
        .iter()
        .find(|k| k.as_bytes() == name.as_slice())
        .copied()
        .unwrap_or("GameBase");
    if let Some(env) = env {
        if let Some(maker) = vm.extension::<GameTickers>()
            && let Some(ticker) = (maker.0)(class, id, &env)
        {
            return ticker;
        }
        match class {
            "Player" | "AIPlayer" => {
                return Box::new(super::player::tick::PlayerTicker { id, env, class });
            }
            "Item" => return Box::new(super::item::ItemTicker { id, env }),
            "WheeledVehicle" | "FlyingVehicle" => {
                return Box::new(super::vehicle_tick::VehicleTicker { id, env, class });
            }
            _ => {}
        }
    }
    Box::new(NoopTicker { class })
}
