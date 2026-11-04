//! The host side of the simulation: the server's process list and the pass that ticks it (`::sim` is the pure part).
//!
//! **What is in the list.** Every `GameBase` object whose `onAdd` took its datablock: the host's `GameBase` class
//! adds it and, when it leaves, removes it. What an object is ticked with is a [`Ticker`], chosen by its class
//! ([`adapters::ticker_for`]): the `Camera`, the `Player` and `AIPlayer`, and the `Item` and the
//! `Projectile` have real ones, every other class a no-op.
//!
//! **The pass** ([`pass::advance_objects`], once per 32 ms step) is run by the server's tick loop whether or not a network
//! host exists. A connection's control object is ticked by the host's control branch (its moves, `net::pump`); every other
//! object gets `processTick(NULL)`, held against deletion while it runs (a `delete` of it waits for its tick to return, a
//! `delete` of any other object is carried out at once and takes it out of the pass).
//!
//! **Cost.** One step of the run's budget per object visited ([`pass::Outcome::visits`]); the objects' own script calls are
//! charged as they are elsewhere.
//!
//! **The classes.** The Player glue ([`player`]: the state, the tick over `::sim::player`, the skeleton, the methods), two
//! pieces every `ShapeBase` shares: [`mount`] (the mount links and their methods) and [`vitals`] (damage, energy, their
//! scripts), the Item ([`item`]) over the same borrow rule ([`hosted`]; a host's Projectile uses it too), and the
//! WheeledVehicle and FlyingVehicle ([`vehicle`], [`vehicle_tick`], [`vehicle_methods`]).
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/gameBase/processList.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.);
//! `T3D/gameBase/gameProcess.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

pub mod adapters;
pub mod hosted;
pub mod image;
pub mod item;
pub mod mount;
pub mod pass;
pub mod player;
pub mod vehicle;
pub mod vehicle_methods;
pub mod vehicle_tick;
pub mod vitals;

use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

use ::sim::Ticking;
use ::sim::process::ProcessList;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use crate::scene::camera_tick::TriggerCalls;

/// What a tick needs from the host: the VM, and the way `onTrigger` reaches the script (the network host charges and limits
/// these).
pub struct TickCx<'a> {
    pub vm: &'a mut Vm,
    pub calls: &'a mut dyn TriggerCalls,
}

/// What an object is ticked with.
pub type Ticker = Box<dyn for<'a> Ticking<TickCx<'a>>>;

/// The server's process list, shared by the class behaviours that fill it and the pass that walks it.
#[derive(Clone, Default)]
pub struct ProcessHandle(
    Rc<RefCell<ProcessList<Ticker>>>,
    Rc<RefCell<Option<Rc<player::tick::SimEnv>>>>,
);

impl ProcessHandle {
    pub fn new() -> ProcessHandle {
        ProcessHandle::default()
    }

    pub fn borrow(&self) -> Ref<'_, ProcessList<Ticker>> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, ProcessList<Ticker>> {
        self.0.borrow_mut()
    }

    /// The world and shapes the Players tick in (set once by the host; a list without it gives Players no tick).
    pub fn set_env(&self, env: Rc<player::tick::SimEnv>) {
        *self.1.borrow_mut() = Some(env);
    }

    /// The world and shapes the movers tick in, once set.
    pub fn env(&self) -> Option<Rc<player::tick::SimEnv>> {
        self.1.borrow().clone()
    }

    /// `GameBase::onAdd`'s part: the object enters the list at the tail with the ticker its class has.
    pub fn register(&self, vm: &Vm, id: ObjId) {
        let env = self.1.borrow().clone();
        let ticker = adapters::ticker_for(vm, id, env);
        let (class, after) = (ticker.class(), ticker.process_after());
        self.0.borrow_mut().add(id, class, ticker, after);
    }

    /// `GameBase::onRemove`'s part.
    pub fn unregister(&self, id: ObjId) {
        // The ticker may be out for a tick: the pass drops it when it finds the object gone.
        self.0.borrow_mut().remove(id);
    }

    /// `GameBase::processAfter`.
    pub fn process_after(&self, id: ObjId, target: Option<ObjId>) {
        self.0.borrow_mut().process_after(id, target);
    }

    /// The edge let go with the list left clean (the Camera's orbit core).
    pub fn clear_after(&self, id: ObjId) {
        self.0.borrow_mut().clear_after(id);
    }

    /// The objects in the order the next pass visits them (for tests).
    pub fn order(&self) -> Vec<ObjId> {
        self.0.borrow_mut().order_list_if_dirty();
        self.0.borrow().order()
    }
}

impl fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.try_borrow() {
            Ok(list) => write!(f, "ProcessHandle({} objects)", list.len()),
            Err(_) => f.write_str("ProcessHandle(busy)"),
        }
    }
}

impl PartialEq for ProcessHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

fn handle_of(vm: &Vm, id: ObjId) -> Option<ProcessHandle> {
    crate::scene::ext::get(vm.objects(), id)?.process.clone()
}

/// The world and shapes the object `id` ticks in: its process list's (none for an object outside a list, or a list without
/// them).
pub fn env_of(vm: &Vm, id: ObjId) -> Option<Rc<player::tick::SimEnv>> {
    handle_of(vm, id)?.env()
}

/// `GameBase::processAfter(target)` for the object `id`; nothing for an object that is not in the list.
pub fn process_after(vm: &Vm, id: ObjId, target: Option<ObjId>) {
    if let Some(h) = handle_of(vm, id) {
        h.process_after(id, target);
    }
}

/// The edge of `id` let go without making the list dirty (`mAfterObject` cleared directly, as in the Camera's orbit setter and
/// in `fxLight::detachFromObject`).
pub fn clear_after(vm: &Vm, id: ObjId) {
    if let Some(h) = handle_of(vm, id) {
        h.clear_after(id);
    }
}
