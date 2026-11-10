//! The demo's Player: TGE's, with the seat rules of a game where E gets out. A seated Player hands the vehicle it drives every
//! trigger of its move, the jump trigger (2) included, which is a WheeledVehicle's brake (TGE keeps that trigger for the Player,
//! which dismounts with it); no trigger of a move dismounts it ([`DISMOUNT_TRIGGER`] is one the demo's keys never set); and no
//! move turns a seated Player, which faces its seat's way, its head level (TGE's passenger turns by its moves' yaw).
//! The vehicle turns its wheels only by the yaw of a move made at the wheel ([`DRIVE_TRIGGER`] held): the moves the client made
//! on foot before it learned it was seated carry the mouse's turn, which would steer. `straightenWheels(%vehicle)` sets a
//! vehicle's wheels straight (a new driver's start); `stopVehicle(%vehicle)` stops it still, its wheels straight and not
//! turning (a buggy put back in its place).
//!
//! The demo's WheeledVehicle ([`SteeredWheels`]) sends its steering whenever it changes: TGE sends a vehicle's steering only with
//! its pose, so a parked buggy's steering wheel, turned by its driver, would stay still for everyone else.
//!
//! The server runs it ([`SeatsPlugin`]) and the client predicts with it ([`SeatedPlayerKind`] through
//! [`torque_example_common::prediction::CubeClient::with_terrain`]), so the client's drive matches the server's.

use std::rc::Rc;

use torque::app::plugins::DefaultPlugins;
use torque::collide::ObjectId;
use torque::engine::{Engine, EngineBuilder, Plugin, PluginId, VmMut};
use torque::game::sim_classes::{PlayerKind, SimClasses};
use torque::script::vm::sim::ObjId;
use torque::script::vm::vm::Vm;
use torque::sim::Move;
use torque::sim::player::{PlayerClass, PlayerData, PlayerHost, PlayerState, TorquePlayer};
use torque::vehicle::state::{VehicleState, net_mask};
use torque::vehicle::wheeled::WheeledClass;

/// The trigger that dismounts a seated Player: one the demo's keys never set (TGE's is the jump trigger, 2).
pub const DISMOUNT_TRIGGER: usize = 5;
/// The trigger a driver's client holds on the moves it makes at the wheel: only their yaw steers.
pub const DRIVE_TRIGGER: usize = 4;

/// The demo's Player class (module docs).
#[derive(Debug, Default)]
pub struct SeatedPlayer;

impl PlayerClass for SeatedPlayer {
    /// TGE's split, with the jump trigger handed to the vehicle the seated Player drives (its brake).
    fn split_control_move(
        &self,
        s: &mut PlayerState,
        ctrl: ObjectId,
        mv: &Move,
        host: &mut dyn PlayerHost,
    ) -> (Move, Move) {
        let (mut own, mut control) = TorquePlayer.split_control_move(s, ctrl, mv, host);
        if s.mount.is_some() {
            control.triggers[2] = mv.triggers[2];
            own.triggers[2] = false;
            if !mv.triggers[DRIVE_TRIGGER] {
                control.yaw = 0.0;
            }
            control.triggers[DRIVE_TRIGGER] = false;
            own.triggers[DRIVE_TRIGGER] = false;
        }
        (own, control)
    }

    fn dismount_trigger(&self) -> usize {
        DISMOUNT_TRIGGER
    }

    /// TGE's, except that a seated Player faces the seat's way, its head level and to the front: no move turns its body, and
    /// its head comes back level. TGE's passenger turns by its moves' yaw and keeps the head's pitch it had on foot, and the
    /// moves its client made on foot before it learned it was seated (the mouse still turning as the click went) reach the
    /// server after the mount, which left the figure sitting askew. The seated look is the client's camera alone.
    fn head_and_rotation(
        &self,
        s: &mut PlayerState,
        db: &PlayerData,
        mv: &Move,
        host: &mut dyn PlayerHost,
    ) {
        if s.mount.is_some() {
            let mv = Move {
                yaw: 0.0,
                pitch: -s.head[0],
                ..*mv
            };
            TorquePlayer.head_and_rotation(s, db, &mv, host);
        } else {
            TorquePlayer.head_and_rotation(s, db, mv, host);
        }
    }
}

/// The demo's WheeledVehicle: TGE's, its steering sent to the other clients whenever a move changes it (module docs).
#[derive(Debug, Default)]
pub struct SteeredWheels;

impl WheeledClass for SteeredWheels {
    fn after_move(&self, s: &mut VehicleState, _mv: &torque::vehicle::Move) {
        if s.steering_delta != [0.0; 2] {
            s.set_mask(net_mask::POSE);
        }
    }
}

/// [`SeatedPlayer`] as the class a `PlayerData` makes.
#[derive(Debug, Default)]
pub struct SeatedPlayerKind;

impl PlayerKind for SeatedPlayerKind {
    fn class(&self) -> Rc<dyn PlayerClass> {
        Rc::new(SeatedPlayer)
    }
}

/// The demo's simulated classes: TGE's, its Players [`SeatedPlayer`]s.
#[derive(Debug, Default)]
pub struct DemoClasses;

impl SimClasses for DemoClasses {
    fn player_kind(&self, vm: &Vm, db: ObjId) -> Rc<dyn PlayerKind> {
        let _ = (vm, db);
        Rc::new(SeatedPlayerKind)
    }

    fn default_player_kind(&self) -> Rc<dyn PlayerKind> {
        Rc::new(SeatedPlayerKind)
    }

    fn wheeled_class(&self, vm: &Vm, db: ObjId) -> Option<Rc<dyn WheeledClass>> {
        let _ = (vm, db);
        Some(Rc::new(SteeredWheels))
    }
}

/// The demo's server side: its simulated classes ([`DemoClasses`], the engine's `SimClasses` slot, installed before the server
/// game reads them) and the natives its scripts call (`straightenWheels`, `stopVehicle`).
#[derive(Debug, Clone, Copy, Default, PluginId)]
#[plugin(id = "vehicle-demo.seats")]
pub struct SeatsPlugin;

impl Plugin for SeatsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.set_slot::<dyn SimClasses>(Rc::new(DemoClasses));
        engine
            .server_script()
            .natives(torque::natives![straighten_wheels, stop_vehicle]);
    }
}

/// An engine for a server alone: the engine's defaults and [`SeatsPlugin`] (the dedicated server, the page's worker).
pub fn server_engine() -> Result<Engine, String> {
    EngineBuilder::new()
        .add_plugins((DefaultPlugins, SeatsPlugin))
        .build()
        .map_err(|e| e.to_string())
}

/// Sets a vehicle's wheels straight (a new driver's start).
#[torque::function(usage = "straightenWheels(vehicle);")]
fn straighten_wheels(mut vm: VmMut, vehicle: &[u8]) {
    let id = vm.objects().find(vehicle);
    if let Some(s) = id.and_then(|id| torque::game::sim::vehicle::state_mut(&mut vm, id)) {
        s.steering = [0.0; 2];
        s.steering_delta = [0.0; 2];
    }
}

/// Stops a vehicle still, its wheels straight and not turning (a buggy put back in its place).
#[torque::function(usage = "stopVehicle(vehicle);")]
fn stop_vehicle(vm: VmMut, vehicle: &[u8]) {
    use torque::game::sim::vehicle::{self, Body};
    let Some(id) = vm.objects().find(vehicle) else {
        return;
    };
    let vm = vm.into_inner();
    vehicle::set_velocity(vm, id, [0.0; 3]);
    vehicle::set_angular_velocity(vm, id, [0.0; 3]);
    if let Some(s) = vehicle::state_mut(vm, id) {
        s.steering = [0.0; 2];
        s.steering_delta = [0.0; 2];
    }
    if let Some(Body::Wheeled(v)) = vehicle::ext_mut(vm, id).and_then(|e| e.body.as_mut()) {
        for w in &mut v.k.wheels {
            w.avel = 0.0;
        }
    }
}
