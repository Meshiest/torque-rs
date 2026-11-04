//! The ghosting a game's scripts drive: the `NetObject` behaviour that enters every ghostable object in the hub, and the
//! console methods that start and stop a connection's ghosting and give it its control object.
//!
//! | method | does |
//! |---|---|
//! | `GameConnection::activateGhosting()` | [`GhostHub::activate`]: the ghost-always objects' events (and the message that starts them) are posted on the connection |
//! | `GameConnection::resetGhosting()` | [`GhostHub::reset`]: ghosting and scoping off, every ghost dropped, `EndGhosting` posted |
//! | `GameConnection::setControlObject(%obj)` | a ShapeBase by name or id becomes the control object and the scope object (true); anything else changes nothing (false) |
//! | `GameConnection::getControlObject()` | the control object's id, 0 for none |
//!
//! `setControlObject` follows TGE: an object already held changes nothing; else the old control object is let go, whatever
//! controlled the new one (a Camera or a Player holding it) lets it go and another connection that held it loses it, then the
//! connection holds it and scopes from it. Called on a connection the host did not make (a script's `new GameConnection()`),
//! each method answers its neutral value.
//!
//! **`NetObject`.** Each object of a class the codecs can ghost is entered in the hub when it is added ([`GhostHub::added`]:
//! a ScopeAlways one joins `GhostAlwaysSet` and every ghosting connection scopes it) and gets the hub as its link
//! ([`torque_game::ghost::GhostLink`]), so the game classes' mask writes reach the connections; on removal every connection
//! drops its ghost ([`GhostHub::removed`]).
//!
//! Credit: the methods follow TGE as Torque3D, its MIT-licensed release, keeps them: `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC), `sim/netObject.cpp` and `T3D/gameBase/gameConnection.cpp` (each Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::cell::RefCell;
use std::rc::Rc;

use torque_net::handshake::ConnId;
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::text::until_nul;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::conn::{post_events, register};
use super::ghosts::{GhostHub, SourceEnv};
use super::hostevent::HostEvent;
use super::{NetGame, NetShared};
use torque_game::scene::camera::is_class;
use torque_game::scene::ext;

/// The `NetObject` behaviour over the host's hub.
struct NetObjectClass<G: NetGame> {
    hub: GhostHub<G::Ghosts>,
}

impl<G: NetGame> EngineClass for NetObjectClass<G> {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        self.hub.added(vm, id);
        if self.hub.knows(id) {
            let now = vm.sim_time();
            if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
                e.ghost = Some(Rc::new(self.hub.clone()));
                e.created_ms = now;
            }
        }
        Ok(true)
    }

    fn on_remove(&self, _vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.hub.removed(id);
        Ok(())
    }
}

/// The connection of the object a method was called on, and the hub, when the host made that connection.
fn this_conn<G: NetGame>(
    shared: &Rc<RefCell<NetShared<G>>>,
    c: &NativeCall,
) -> Option<(ObjId, ConnId, GhostHub<G::Ghosts>)> {
    let this = c.this_object?;
    let sh = shared.borrow();
    let conn = sh.conns.get(&this)?.conn?;
    let hub = sh.ghosts.clone()?;
    Some((this, conn, hub))
}

/// Gives `NetObject` its behaviour and registers the ghosting methods. [`super::attach_host`] calls it.
pub fn install<G: NetGame>(vm: &mut Vm, shared: &Rc<RefCell<NetShared<G>>>) {
    let Some(hub) = shared.borrow().ghosts.clone() else {
        return;
    };
    vm.set_class_behaviour("NetObject", Rc::new(NetObjectClass::<G> { hub }));

    register(
        vm,
        "GameConnection",
        "activateGhosting",
        shared,
        |vm, c, s| {
            let Some((this, conn, hub)) = this_conn(s, c) else {
                return Native::Void;
            };
            let table = s.borrow().string_table();
            let env = SourceEnv {
                vm,
                table: Some(&table),
                ghost_bits: hub.conn_ghost_bits(conn),
            };
            match hub.activate(env, conn) {
                Ok(events) => {
                    if !vm.charge_work(1 + events.len() as u64) {
                        return Native::Fail(VmError::StepLimit);
                    }
                    post_events(
                        vm,
                        s,
                        this,
                        events.into_iter().map(HostEvent::Ghost).collect(),
                    )
                }
                // A second activation without a reset: nothing happens.
                Err(_) => Native::Void,
            }
        },
    );
    register(vm, "GameConnection", "resetGhosting", shared, |vm, c, s| {
        let Some((this, conn, hub)) = this_conn(s, c) else {
            return Native::Void;
        };
        match hub.reset(conn) {
            Some(event) => post_events(vm, s, this, vec![HostEvent::Ghost(event)]),
            None => Native::Void,
        }
    });
    register(
        vm,
        "GameConnection",
        "setControlObject",
        shared,
        |vm, c, s| {
            let Some((_, conn, hub)) = this_conn(s, c) else {
                return Native::Bool(false);
            };
            let target = c.argv.get(2).map_or(&b""[..], |a| until_nul(a));
            let Some(obj) = vm
                .objects()
                .find(target)
                .filter(|o| is_class(vm, *o, "ShapeBase"))
            else {
                return Native::Bool(false);
            };
            if hub.control_of(conn) != Some(obj) {
                // Nothing else may control the object: what holds it lets it go, and so does another connection.
                if let Some(holder) = torque_game::sim::mount::controlling_object(vm, obj) {
                    torque_game::sim::player::methods::release_holder(vm, holder);
                }
                hub.adopt_control(conn, obj);
            }
            Native::Bool(true)
        },
    );
    register(
        vm,
        "GameConnection",
        "getControlObject",
        shared,
        |_, c, s| {
            let obj = this_conn(s, c).and_then(|(_, conn, hub)| hub.control_of(conn));
            Native::Int(obj.map_or(0, |o| i32::try_from(o).unwrap_or(0)))
        },
    );
}
