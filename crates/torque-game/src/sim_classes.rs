//! The host's subclasses of the simulated classes ([`SimClasses`], a `Vm` extension): the Player class a `PlayerData` makes,
//! with the per-object state the subclass keeps on the Player ([`PlayerKind`]). Without a host's, they are TGE's
//! (`TorquePlayer`).

use std::any::Any;
use std::fmt::Debug;
use std::rc::Rc;

use ::sim::image::RayCaster;
use ::sim::player::{PlayerClass, PlayerData, PlayerState, TorquePlayer};
use collide::convex::ConvexSource;
use collide::math::{Box3, F3, Matrix};
use collide::{ObjectId, ObjectView};
use shapes::instance::ThreadId;
use shapes::preload::SlotRule;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;
use vehicle::VehicleState;
use vehicle::wheeled::{Wheeled, WheeledClass};

use crate::scene::shapebase::{ActionTable, ShapeBody, TGE_ACTIONS};

/// A `PlayerData`'s Player class as the host makes it: the class the step runs, and what its subclass keeps on each Player.
pub trait PlayerKind: Debug {
    /// The class the step runs.
    fn class(&self) -> Rc<dyn PlayerClass>;

    /// A new Player's state (`Player::onNewDataBlock`'s first datablock).
    fn new_state(&self, id: ObjectId, db: &PlayerData, pos: F3, yaw: f32) -> PlayerState {
        PlayerState::new(id, db, pos, yaw)
    }

    /// A later datablock: what the subclass reads of it.
    fn datablock_changed(&self, s: &mut PlayerState, db: &PlayerData) {
        let _ = (s, db);
    }

    /// The scaled box from the object box and the scale.
    fn update_scaled_box(&self, s: &mut PlayerState) {
        s.update_scaled_box();
    }

    /// The state an `AIPlayer` adds.
    fn make_ai(&self, s: &mut PlayerState) {
        let _ = s;
    }

    /// Whether the Player is crouched.
    fn crouched(&self, s: &PlayerState) -> bool {
        let _ = s;
        false
    }

    /// Whether the Player is jetting.
    fn jetting(&self, s: &PlayerState) -> bool {
        let _ = s;
        false
    }

    /// `pickActionAnimation` for a Player in its move state and not mounted: the action to set and whether it plays forward,
    /// or None to keep the one it has. TGE's by default ([`pick_action`]).
    fn pick_action(&self, pick: &ActionPick<'_>) -> Option<(u32, bool)> {
        Some(pick_action(pick, self.crouched(pick.s)))
    }

    /// `advanceThreads(dt)`'s step of the threads the class added
    /// ([`SimClasses::player_class_threads`]; `threads` by their numbers), after the action thread's: the shape work it did.
    /// TGE's adds none, so does nothing.
    fn advance_class_threads(
        &self,
        body: &mut ShapeBody,
        threads: &[Option<ThreadId>],
        dt: f32,
    ) -> u64 {
        let _ = (body, threads, dt);
        0
    }

    /// The end of a step: the method the Player is then told of (`%obj.<name>()`), if any. None by default.
    fn after_step(&self, s: &mut PlayerState) -> Option<&'static [u8]> {
        let _ = s;
        None
    }

    /// Before a step: what the class reads from outside the Player, `client` being its controlling client's connection object
    /// (none without one). Nothing by default.
    fn before_step(&self, vm: &Vm, me: ObjId, client: Option<ObjId>, s: &mut PlayerState) {
        let _ = (vm, me, client, s);
    }
}

/// What `pickActionAnimation` picks from ([`PlayerKind::pick_action`]).
#[derive(Debug)]
pub struct ActionPick<'a> {
    pub s: &'a PlayerState,
    /// The class's action table.
    pub table: &'a ActionTable,
    /// The velocity in object space.
    pub velocity: F3,
    /// The ground directions of the moving actions 1..3 that have a sequence and a speed (none at 0, the root).
    pub moves: [Option<F3>; 4],
    /// `isFirstPerson`: the Player is a client's control object, seen in first person.
    pub first_person: bool,
    /// The action thread holds at its end.
    pub held: bool,
}

/// `sContactTickTime`: the ticks without a contact after which the moving actions give way to `root`.
pub const CONTACT_TICK_TIME: u32 = 30;

/// TGE's `pickActionAnimation`: falling plays the table's fall action; without a contact for [`CONTACT_TICK_TIME`] ticks,
/// `root`; otherwise the moving action whose ground direction best fits the velocity past 0.1 (`mDot`), side also backwards
/// (`forward` false). A class with crouching actions (`crouched`) moves 1..3 onto them.
pub fn pick_action(pick: &ActionPick<'_>, crouched: bool) -> (u32, bool) {
    const SIDE: usize = 3;
    let (s, table) = (pick.s, pick.table);
    let (mut action, mut forward) = (0u32, true);
    if s.falling {
        action = table.fall;
    } else if s.contact_timer < CONTACT_TICK_TIME {
        let lib = crate::library::get();
        if (lib.moving)(pick.velocity) {
            let mut cur_max = 0.1f32;
            for (i, dir) in pick.moves.iter().enumerate().skip(1) {
                let Some(dir) = *dir else {
                    continue;
                };
                let d = (lib.action_dot)(pick.velocity, dir);
                if d > cur_max {
                    (cur_max, action, forward) = (d, i as u32, true);
                } else if i == SIDE && -d > cur_max {
                    (cur_max, action, forward) = (-d, i as u32, false);
                }
            }
        }
        if let Some(crouch) = table.crouch
            && crouched
            && (1..=3).contains(&action)
        {
            action += crouch - 1;
        }
    }
    (action, forward)
}

/// TGE's Player.
#[derive(Debug, Default)]
pub struct TorquePlayerKind;

impl PlayerKind for TorquePlayerKind {
    fn class(&self) -> Rc<dyn PlayerClass> {
        Rc::new(TorquePlayer)
    }
}

/// The `Vm` a class's host lends a subclass's hook during a step: read it as the step holds it, or take it whole (the step's
/// world is dropped first, as a host does before any write).
pub trait VmAccess {
    fn read_vm(&self, f: &mut dyn FnMut(&Vm));
    fn write_vm(&mut self, f: &mut dyn FnMut(&mut Vm));
}

/// The host's simulated classes.
pub trait SimClasses {
    /// The Player class `PlayerData` `db` makes (the host's subclass reads its own fields of the datablock).
    fn player_kind(&self, vm: &Vm, db: ObjId) -> Rc<dyn PlayerKind> {
        let _ = (vm, db);
        Rc::new(TorquePlayerKind)
    }

    /// The class of a Player whose datablock is not known.
    fn default_player_kind(&self) -> Rc<dyn PlayerKind> {
        Rc::new(TorquePlayerKind)
    }

    /// The Player's action table (TGE's: eight fixed actions).
    fn action_table(&self) -> &'static ActionTable {
        &TGE_ACTIONS
    }

    /// The threads the Player class adds in `onNewDataBlock` to Player `me`'s new shape instance, after the `head` and
    /// `headside` threads and before the action thread, by their numbers (`Thread::Class(n)` is the n-th). TGE's adds none.
    fn player_class_threads(
        &self,
        vm: &Vm,
        me: ObjId,
        body: &mut ShapeBody,
    ) -> Vec<Option<ThreadId>> {
        let _ = (vm, me, body);
        Vec::new()
    }

    /// The datablock field `onNewDataBlock` sizes Player `me`'s object box from: TGE's `boundingBox`.
    fn player_box_field(&self, vm: &Vm, me: ObjId) -> &'static [u8] {
        let _ = (vm, me);
        b"boundingBox"
    }

    /// `PlayerData::preload`'s products.
    fn player_preload(&self, d: &mut PlayerData) {
        d.preload();
    }

    /// A question the host's Player subclass asks its host (`PlayerHost::class_query`), answered in place; `me` is the Player.
    /// TGE's Player asks none.
    fn player_query(&self, vm: &mut dyn VmAccess, me: ObjId, query: &mut dyn Any) {
        let _ = (vm, me, query);
    }

    /// The rule a loaded shape's collision and line-of-sight details are picked by (`Collision-N`, `LOS-N`); TGE's by default.
    fn shape_slots(&self) -> SlotRule {
        shapes::preload::collision_slots
    }

    /// The convex a Player gives the collision queries (`Player::buildConvex`): TGE's builds its box whenever it has a shape.
    fn player_convex(&self, mounted: bool, has_shape: bool) -> ConvexSource {
        let _ = mounted;
        ConvexSource::Player { has_shape }
    }

    /// `getMuzzlePoint`'s point (`RayCaster::muzzle_point`): TGE's keeps `muzzle`.
    fn muzzle_point(
        &self,
        rays: &mut dyn RayCaster,
        s: &PlayerState,
        mount: F3,
        muzzle: F3,
        player: bool,
    ) -> F3 {
        let _ = (rays, s, mount, player);
        muzzle
    }

    /// The corrected aim (`RayCaster::corrected_aim`): TGE's aims from `muzzle`.
    fn corrected_aim(
        &self,
        rays: &mut dyn RayCaster,
        eye: &Matrix,
        muzzle: F3,
        point: F3,
    ) -> Option<F3> {
        let _ = point;
        ::sim::image::muzzle::corrected_aim(rays, eye, muzzle)
    }

    /// The WheeledVehicle subclass `WheeledVehicleData` `db` makes; none runs TGE's.
    fn wheeled_class(&self, vm: &Vm, db: ObjId) -> Option<Rc<dyn WheeledClass>> {
        let _ = (vm, db);
        None
    }

    /// Before a vehicle's step: what its class reads from outside the vehicle, `client` being its controlling client's
    /// connection object (none without one). Nothing by default.
    fn before_vehicle_step(&self, vm: &Vm, me: ObjId, client: Option<ObjId>, s: &mut VehicleState) {
        let _ = (vm, me, client, s);
    }

    /// The datablock callback a WheeledVehicle's wheel contacts call (`%db.<name>(%obj, %obj)`) after its body contact, if any.
    fn wheeled_contact_callback(&self, wheels: &Wheeled) -> Option<&'static [u8]> {
        let _ = wheels;
        None
    }

    /// The scale of the object box `Player::checkDismountPosition` tests about the dismount point: TGE's whole box (1).
    fn dismount_box_scale(&self) -> f32 {
        1.0
    }

    /// `findContact`'s touch of `obj` (`PlayerHost::contact_touch`): true queues it. TGE has no such touch.
    fn contact_touch(
        &self,
        vm: &mut dyn VmAccess,
        s: &PlayerState,
        obj: ObjectId,
        view: &ObjectView,
        query: &Box3,
        queue_full: bool,
    ) -> bool {
        let _ = (vm, s, obj, view, query, queue_full);
        false
    }

    /// The script call for a touch [`contact_touch`](Self::contact_touch) queued, run after the Player's step: `player` touched
    /// `obj` (which still exists).
    fn touched(&self, vm: &mut Vm, player: ObjId, obj: ObjId) -> Result<(), ts_vm::vm::VmError> {
        let _ = (vm, player, obj);
        Ok(())
    }
}

/// TGE's classes.
pub struct TorqueClasses;

impl SimClasses for TorqueClasses {}

struct Installed(Rc<dyn SimClasses>);

/// The host's classes for `vm` from now on.
pub fn set_sim_classes(vm: &mut Vm, classes: Rc<dyn SimClasses>) {
    vm.set_extension(Rc::new(Installed(classes)));
}

/// The classes `vm`'s game installed, else TGE's.
pub fn sim_classes(vm: &Vm) -> Rc<dyn SimClasses> {
    vm.extension::<Installed>().map_or_else(
        || Rc::new(TorqueClasses) as Rc<dyn SimClasses>,
        |i| Rc::clone(&i.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(s: &PlayerState, velocity: F3) -> (u32, bool) {
        let p = ActionPick {
            s,
            table: &TGE_ACTIONS,
            velocity,
            moves: [
                None,
                Some([0.0, 1.0, 0.0]),
                Some([0.0, -1.0, 0.0]),
                Some([-1.0, 0.0, 0.0]),
            ],
            first_person: false,
            held: false,
        };
        TorquePlayerKind.pick_action(&p).unwrap()
    }

    /// TGE's pick: the best fitting moving action past 0.1, side also backwards; root without a recent contact; fall falling.
    #[test]
    fn tges_pick_is_the_best_fitting_moving_action() {
        let mut s = PlayerState::new(ObjectId(1), &PlayerData::default(), [0.0; 3], 0.0);
        assert_eq!(pick(&s, [0.0, 3.0, 0.0]), (1, true));
        assert_eq!(pick(&s, [0.0, -3.0, 0.0]), (2, true));
        assert_eq!(pick(&s, [-3.0, 0.5, 0.0]), (3, true));
        assert_eq!(pick(&s, [3.0, 0.5, 0.0]), (3, false));
        assert_eq!(pick(&s, [0.05, 0.05, 0.0]), (0, true));
        s.contact_timer = CONTACT_TICK_TIME;
        assert_eq!(pick(&s, [0.0, 3.0, 0.0]), (0, true));
        s.falling = true;
        assert_eq!(pick(&s, [0.0, 3.0, 0.0]), (TGE_ACTIONS.fall, true));
    }
}
