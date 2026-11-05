//! The demo's Player: TGE's, with the seat rules of a game where E gets out. A seated Player hands the vehicle it drives every
//! trigger of its move, the jump trigger (2) included, which is a WheeledVehicle's brake (TGE keeps that trigger for the Player,
//! which dismounts with it); and no trigger of a move dismounts it ([`DISMOUNT_TRIGGER`] is one the demo's keys never set).
//!
//! The server runs it ([`install`]) and the client predicts with it ([`SeatedPlayerKind`] through
//! [`torque_example_common::prediction::CubeClient::with_terrain`]), so the client's drive matches the server's.

use std::rc::Rc;

use torque::collide::ObjectId;
use torque::game::sim_classes::{PlayerKind, SimClasses, set_sim_classes};
use torque::script::vm::sim::ObjId;
use torque::script::vm::vm::Vm;
use torque::sim::Move;
use torque::sim::player::{PlayerClass, PlayerHost, PlayerState, TorquePlayer};

/// The trigger that dismounts a seated Player: one the demo's keys never set (TGE's is the jump trigger, 2).
pub const DISMOUNT_TRIGGER: usize = 5;

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
        }
        (own, control)
    }

    fn dismount_trigger(&self) -> usize {
        DISMOUNT_TRIGGER
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
}

/// Gives a server's script engine the demo's classes (before its game classes are installed).
pub fn install(vm: &mut Vm) {
    set_sim_classes(vm, Rc::new(DemoClasses));
}
