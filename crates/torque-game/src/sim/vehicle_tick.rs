//! A vehicle's tick on the host: [`VehicleTicker`] (the process list's entry), [`tick_controlled`] (the driver's hand-off,
//! `Player::processTick`'s call of its control object's) and [`HostImpl`], the `vehicle::VehicleHost` over the `Vm`, the
//! container and the script callbacks.
//!
//! **Who ticks a vehicle.** `ShapeBase::setControllingObject` clears the object's process-tick flag while
//! something controls it: a vehicle a Player drives is ticked only by that Player, once per move the Player runs, with the
//! driver's filtered move (`sim::player`'s `drive_control_object`: triggers 0, 3 and 4 zeroed); the process list passes it by.
//! Every other vehicle gets `processTick(NULL)` from the list, and a vehicle that is a connection's own control object gets the
//! connection's moves.
//!
//! **The borrow rule** is the Player's and the Item's (the `ContainerWorld` contract): the container is built the first time a
//! phase asks for the world and dropped before any script runs; around each callback the vehicle's state is parked in its ext
//! (with its damage and energy words given back to [`super::vitals`]), so the methods the script calls find it, then taken
//! back. The vehicle is held for its tick (an object's own delete waits until its tick returns); any other object a script
//! deletes is gone at once.
//!
//! **Callbacks** at the vehicle's call sites: `onTrigger` (`ShapeBase::processTick`, through the tick's trigger calls), the
//! damage step's level update and `onDamage`, `notifyCollision`'s pair (`Vehicle::onCollision`: `%db.onCollision(%obj, %col,
//! "%f %f %f", "%f")`, then the push onto a vehicle it hit; the other object's `onCollision` with this one, a vehicle's push
//! landing back here), `onImpact(%obj, "0", vec, speed)`, `onEnterLiquid`/`onLeaveLiquid`, and the wheel-contact callback a
//! WheeledVehicle's class names
//! ([`SimClasses::wheeled_contact_callback`](crate::sim_classes::SimClasses::wheeled_contact_callback)).
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp` and `T3D/vehicles/flyingVehicle.cpp` (Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.

use std::rc::Rc;

use collide::math::{Box3, F3};
use collide::types;
use collide::{ObjectId, World};
use ts_vm::sim::ObjId;
use ts_vm::text::format_g;
use ts_vm::vm::{Vm, VmError};
use vehicle::host::{RayHit, ScanHit, VehicleHost};
use vehicle::{Move, VehicleState, process_tick};

use super::TickCx;
use super::hosted::{Slot, call_db, switch_collision, vector_args};
use super::player::tick::SimEnv;
use super::vehicle::{
    Body, Data, data, env_of, ext_mut, flush_mask, put_vitals, spring_data, state_mut, take_fields,
    tire_data, write_copies,
};
use crate::scene::access::object_field;
use crate::scene::camera::is_class;
use crate::scene::camera_tick::TriggerCalls;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::object as scene;

/// The objects the scan of one sub-step looks at, at most (a bound on the callback list).
pub const SIM_VEHICLE_SCAN: usize = 256;

/// The `vehicle` form of a `sim` move.
pub fn vehicle_move(m: &::sim::Move) -> Move {
    Move {
        x: m.x,
        y: m.y,
        z: m.z,
        yaw: m.yaw,
        pitch: m.pitch,
        roll: m.roll,
        free_look: m.free_look,
        triggers: m.triggers,
    }
}

/// A vehicle's entry in the process list.
pub struct VehicleTicker {
    pub id: ObjId,
    pub env: Rc<SimEnv>,
    pub class: &'static str,
}

impl ::sim::Ticking<TickCx<'_>> for VehicleTicker {
    fn class(&self) -> &'static str {
        self.class
    }

    fn process_tick(&mut self, cx: &mut TickCx<'_>, mv: Option<&::sim::Move>) {
        // The process-tick byte is clear while something controls the vehicle: its controller ticks it.
        if mv.is_none() && super::mount::controlling_object(cx.vm, self.id).is_some() {
            return;
        }
        let mv = mv.map(vehicle_move);
        let _ = tick(cx.vm, cx.calls, &self.env, self.id, mv.as_ref());
    }
}

/// The driver's hand-off (`Player::processTick` -> the control object's) for a vehicle control object: one tick
/// with the filtered move (`None` for a disabled driver's `&NullMove`). Nothing for an object that is not a vehicle with its
/// state.
pub fn tick_controlled(
    vm: &mut Vm,
    calls: &mut dyn TriggerCalls,
    id: ObjId,
    mv: Option<&::sim::Move>,
) -> Result<(), VmError> {
    if !super::vehicle::is_vehicle(vm, id) {
        return Ok(());
    }
    let Some(env) = env_of(vm, id) else {
        return Ok(());
    };
    let mv = mv.map(vehicle_move);
    tick(vm, calls, &env, id, mv.as_ref())
}

/// The tires' and springs' fields of the wheels again (read through the datablocks every step).
fn refresh_wheels(vm: &Vm, body: &mut Body) {
    let Body::Wheeled(v) = body else {
        return;
    };
    for w in v.k.wheels.iter_mut() {
        if let Some((id, t)) = w.tire.as_mut()
            && vm.objects().contains(id.0)
        {
            *t = tire_data(vm, id.0);
        }
        if let Some((id, s)) = w.spring.as_mut()
            && vm.objects().contains(id.0)
        {
            *s = spring_data(vm, id.0);
        }
    }
}

/// One tick of vehicle `id` (`processTick` through `vehicle`), `mv` the move (`None`: `&NullMove`). A spent work
/// budget stops the step where it was (the state as far as it got); the first script error is returned.
pub fn tick(
    vm: &mut Vm,
    calls: &mut dyn TriggerCalls,
    env: &SimEnv,
    id: ObjId,
    mv: Option<&Move>,
) -> Result<(), VmError> {
    if !vm.objects().contains(id) {
        return Ok(());
    }
    let Some(d) = data(vm, id) else {
        return Ok(());
    };
    let Some(mut body) = ext_mut(vm, id).and_then(|e| e.body.take()) else {
        return Ok(());
    };
    take_fields(vm, id, body.s_mut());
    refresh_wheels(vm, &mut body);
    body.s_mut().mask_bits = 0;
    vm.hold(id);
    let damage_before = body.s().damage;
    let solver = object_field(vm, id, b"dataBlock")
        .and_then(|db| crate::sim_classes::sim_classes(vm).body_solver(vm, db));
    let mut host = HostImpl {
        slot: Slot::Vm(vm),
        env,
        calls,
        id,
        spare: None,
        error: None,
        moved: false,
        damage_before,
        class: None,
        solver,
    };
    let _ = match (&mut body, &d) {
        (Body::Wheeled(v), Data::Wheeled(d, c)) => {
            host.class = c.clone();
            process_tick(&mut **v, d, mv, &mut host)
        }
        (Body::Flying(v), Data::Flying(d)) => process_tick(v, d, mv, &mut host),
        _ => Ok(()),
    };
    if host.moved {
        host.flush_move(body.s());
    }
    let error = host.error.take();
    let vm = host.slot.into_vm();
    put_vitals(vm, id, body.s());
    if let Some(e) = ext_mut(vm, id) {
        e.body = Some(body);
    }
    flush_mask(vm, id);
    let released = vm.release(id);
    match error {
        Some(e) => Err(e),
        None => released,
    }
}

/// The host of one vehicle's tick.
pub struct HostImpl<'a> {
    slot: Slot<'a>,
    env: &'a SimEnv,
    calls: &'a mut dyn TriggerCalls,
    id: ObjId,
    /// The box the state swaps with while it is parked.
    spare: Option<Box<VehicleState>>,
    /// The first error a callback's script returned (the step goes on).
    pub error: Option<VmError>,
    /// `setPosition` moved the vehicle since its copies were last written.
    moved: bool,
    /// The damage before the damage step (`onDamage`'s difference).
    damage_before: f32,
    /// A WheeledVehicle's subclass (the host's), for its step.
    class: Option<Rc<dyn vehicle::wheeled::WheeledClass>>,
    /// The datablock's solver for the body, handed to the step once ([`SimClasses::body_solver`](crate::sim_classes::SimClasses::body_solver)).
    solver: Option<Box<dyn vehicle::BodySolver>>,
}

impl HostImpl<'_> {
    fn note(&mut self, r: Result<(), VmError>) {
        if let Err(e) = r
            && self.error.is_none()
        {
            self.error = Some(e);
        }
    }

    /// Writes the vehicle's copies (matrix, world box, bins) from `s`.
    fn flush_move(&mut self, s: &VehicleState) {
        let id = self.id;
        let container = Rc::clone(&self.env.container);
        let vm = self.slot.vm();
        write_copies(vm, id, s);
        scene::check_bins(vm, &container, id);
        self.moved = false;
    }

    /// Runs script with the vehicle's state parked where its methods find it (and its damage and energy words where theirs do).
    fn script(
        &mut self,
        s: &mut VehicleState,
        f: impl FnOnce(&mut Vm, &mut dyn TriggerCalls, ObjId) -> Result<(), VmError>,
    ) {
        if self.moved {
            self.flush_move(s);
        }
        let id = self.id;
        let mut parked = self.spare.take().unwrap_or_else(|| Box::new(s.clone()));
        std::mem::swap(&mut *parked, s);
        let vm = self.slot.vm();
        put_vitals(vm, id, &parked);
        let mut kept = None;
        match ext_mut(vm, id) {
            Some(e) => e.parked = Some(parked),
            None => kept = Some(parked),
        }
        let r = f(vm, &mut *self.calls, id);
        let back = kept.or_else(|| ext_mut(vm, id).and_then(|e| e.parked.take()));
        if let Some(mut b) = back {
            std::mem::swap(&mut *b, s);
            self.spare = Some(b);
        }
        // The callback may have moved it (`setTransform`: its copies were written by that call) or changed its words.
        take_fields(vm, id, s);
        self.damage_before = s.damage;
        self.note(r);
    }

    /// This vehicle and everything mounted on it, at any depth (`disableCollision`'s recursion); charged per object.
    fn with_riders(&mut self) -> Vec<ObjId> {
        let vm = self.slot.vm();
        let mut out = vec![self.id];
        let mut i = 0;
        while i < out.len() && out.len() <= super::mount::SIM_MOUNT_CHAIN {
            let riders = super::mount::riders(vm, out[i]).to_vec();
            if !vm.charge_work(1 + riders.len() as u64) {
                break;
            }
            for r in riders {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
            i += 1;
        }
        out
    }

    fn switch(&mut self, disable: bool) {
        for o in self.with_riders() {
            switch_collision(self.slot.vm(), o, disable);
        }
    }
}

impl<'a> VehicleHost for HostImpl<'a> {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.env.masks
    }

    fn world(&mut self) -> &mut dyn World {
        self.slot.world(self.env)
    }

    fn sim_time(&self) -> u32 {
        self.slot.read(|vm| vm.sim_time())
    }

    fn disable_collision(&mut self, _s: &VehicleState) {
        self.switch(true);
    }

    fn enable_collision(&mut self, _s: &VehicleState) {
        self.switch(false);
    }

    fn cast_ray(&mut self, start: F3, end: F3, ray_mask: u32) -> Option<RayHit> {
        let env = self.env;
        let hit = crate::scene::container::container_ray(
            self.slot.vm(),
            &env.container,
            Some(&env.shapes),
            start,
            end,
            ray_mask,
        );
        match hit {
            Ok(h) => h.map(|h| RayHit {
                t: h.t,
                point: h.point,
                normal: h.normal,
                material: 0,
                object: Some(ObjectId(h.object)),
            }),
            Err(e) => {
                self.note(Err(e));
                None
            }
        }
    }

    fn scan(&mut self, _s: &mut VehicleState, query: &Box3, out: &mut Vec<ScanHit>) {
        let me = ObjectId(self.id);
        let mut ids = Vec::new();
        let scan = self.type_masks().vehicle_scan;
        if self
            .world()
            .find_objects(query, scan, Some(me), SIM_VEHICLE_SCAN, &mut ids)
            .is_err()
        {
            return;
        }
        self.slot.read(|vm| {
            for o in ids {
                let m = ext::get(vm.objects(), o.0).map_or(0, |e| e.type_mask);
                // `findCallback`: triggers are the host's (not called yet, listed), corpses and items are queued.
                if m & (types::CORPSE | types::ITEM) != 0 {
                    out.push(ScanHit {
                        object: o,
                        velocity: super::item::object_velocity(vm, o.0),
                    });
                }
            }
        });
    }

    fn object_velocity(&mut self, id: ObjectId) -> F3 {
        self.slot.read(|vm| super::item::object_velocity(vm, id.0))
    }

    fn mount_energy(&mut self, s: &VehicleState) -> f32 {
        let start = s.mount.map(|m| m.0);
        self.slot.read(|vm| {
            let mut at = start;
            for _ in 0..super::vitals::SIM_ENERGY_CHAIN {
                let Some(m) = at.filter(|m| vm.objects().contains(*m)) else {
                    return 0.0;
                };
                if !vm.charge_work(1) {
                    return 0.0;
                }
                if !super::vitals::limits(vm, m).inherit {
                    return super::vitals::get(vm, m).energy;
                }
                at = super::mount::mount_of(vm, m);
            }
            0.0
        })
    }

    fn set_mount_energy(&mut self, s: &VehicleState, e: f32) {
        if let Some(m) = s.mount {
            super::vitals::set_energy_level(self.slot.vm(), m.0, e);
        }
    }

    fn advance_threads(&mut self, s: &mut VehicleState) {
        let id = self.id;
        // Only a slot holding a thread needs the plain `Vm` (taking it drops a container the step built).
        if !self.slot.read(|vm| {
            crate::scene::shapebase::state(vm, id)
                .is_some_and(|st| st.scripts.iter().any(|t| t.thread.is_some()))
        }) {
            return;
        }
        self.script(s, |vm, _, id| {
            crate::scene::shapebase::advance_threads(vm, id)
        });
    }

    fn fade_tick(&mut self, _s: &mut VehicleState) {
        let id = self.id;
        // Only a running fade needs the plain `Vm` (taking it drops a container the step built).
        if self
            .slot
            .read(|vm| crate::scene::shapebase::fade_of(vm, id).fading)
        {
            crate::scene::shapebase::fade_tick(self.slot.vm(), id);
        }
    }

    fn damage_changed(&mut self, s: &mut VehicleState) {
        let (old, new) = (self.damage_before, s.damage);
        self.script(s, |vm, _, id| {
            super::vitals::update_damage_level(vm, id)?;
            ghosts::mark(vm, id, 0x20);
            let delta = (crate::library::get().difference)(new, old);
            call_db(
                vm,
                id,
                b"onDamage",
                &[ts_vm::text::format_fixed(delta, 6).into_bytes()],
            )
        });
    }

    fn on_trigger(&mut self, s: &mut VehicleState, slot: usize, down: bool) {
        self.script(s, |vm, calls, id| {
            match object_field(vm, id, b"dataBlock") {
                Some(db) => calls.on_trigger(vm, db, id, slot, down),
                None => Ok(()),
            }
        });
    }

    fn on_collision(&mut self, s: &mut VehicleState, other: ObjectId, vec: F3) -> bool {
        let mut alive = true;
        let here = s.position();
        self.script(s, |vm, _, id| {
            let [v, len] = vector_args(vec);
            call_db(
                vm,
                id,
                b"onCollision",
                &[other.0.to_string().into_bytes(), v.clone(), len.clone()],
            )?;
            // `Vehicle::onCollision`'s push onto a vehicle it hit.
            if let Some(o) = state_mut(vm, other.0) {
                vehicle::state::collision_push(here, &mut o.rigid, vec);
            }
            alive = vm.objects().contains(id);
            if alive && vm.objects().contains(other.0) && is_class(vm, other.0, "ShapeBase") {
                call_db(
                    vm,
                    other.0,
                    b"onCollision",
                    &[id.to_string().into_bytes(), v, len],
                )?;
                // A vehicle's push lands back on this one (the parked state).
                if let Some(there) = super::vehicle::state(vm, other.0).map(VehicleState::position)
                    && let Some(me) = state_mut(vm, id)
                {
                    vehicle::state::collision_push(there, &mut me.rigid, vec);
                }
            }
            Ok(())
        });
        alive
    }

    fn on_impact(&mut self, s: &mut VehicleState, vec: F3) {
        self.script(s, |vm, _, id| {
            let [v, len] = vector_args(vec);
            call_db(vm, id, b"onImpact", &[b"0".to_vec(), v, len])
        });
    }

    fn on_enter_liquid(&mut self, s: &mut VehicleState, coverage: f32, liquid_type: u32) {
        self.script(s, |vm, _, id| {
            call_db(
                vm,
                id,
                b"onEnterLiquid",
                &[
                    format_g(f64::from(coverage)).into_bytes(),
                    liquid_type.to_string().into_bytes(),
                ],
            )
        });
    }

    fn on_leave_liquid(&mut self, s: &mut VehicleState, liquid_type: u32) {
        self.script(s, |vm, _, id| {
            call_db(
                vm,
                id,
                b"onLeaveLiquid",
                &[liquid_type.to_string().into_bytes()],
            )
        });
    }

    fn wheeled_class(&self) -> Option<Rc<dyn vehicle::wheeled::WheeledClass>> {
        self.class.clone()
    }

    fn body_solver(&mut self) -> Option<Box<dyn vehicle::BodySolver>> {
        self.solver.take()
    }

    /// The class's wheel-contact callback, `%db.<name>(%obj, %obj)`, when it names one for these wheels.
    fn wheeled_body_contact(&mut self, s: &mut VehicleState, wheels: &vehicle::wheeled::Wheeled) {
        let Some(name) = self.env.classes.wheeled_contact_callback(wheels) else {
            return;
        };
        self.script(s, |vm, _, id| {
            call_db(vm, id, name, &[id.to_string().into_bytes()])
        });
    }

    fn moved(&mut self, _s: &mut VehicleState) {
        // The copies are written before the next script and at the end of the tick (the step's own queries leave this vehicle
        // out).
        self.moved = true;
    }
}
