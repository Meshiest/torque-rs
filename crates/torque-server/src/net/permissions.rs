//! Permissions on a server's connections: the key a connection is held by, and the script natives.
//!
//! The server holds a [`Permissions`] as an extension of its script engine (`vm.extension::<Permissions>()`). A connection is held
//! by its auth identity when the ticket gave one, else by the connection itself ([`subject_key`]); connection-keyed grants are
//! forgotten when the connection ends.
//!
//! **Identity.** What a connection is held by comes from the auth provider's check on the server (`Connection::identity`),
//! never from anything the client sent. `%client.getIdentity()` is that key as text (`id:<hex>` for a verified account, the same
//! on every connection of that account; `conn:<n>` for an anonymous player, a new one each time) and
//! `%client.getIdentityName()` the display name the provider gave, or empty. **`getIdentity()` is the raw account id** (in hex):
//! a game that echoes it to other clients (a scoreboard, a chat line) shows them the provider's id, so it should not if the
//! provider's ids are meant to be private. The server's own plugins (props, uploads, permissions) never call it: a script may
//! redefine the method, so they ask the host from Rust ([`ConnectionSubjects`]).
//!
//! **Script.** On a `GameConnection`: `%client.addRole(name)`, `removeRole(name)`, `hasRole(name)`, `getRoles()` (a word list),
//! `grant(grant)`, `revoke(grant)`, `hasPermission(permission)`. The ones that change something answer 1 when they did, 0 when
//! there was nothing to change, and print the reason on the console when they were refused (an unknown role or a name that does
//! not parse, a cap). Globally: `definePermissionRole(name, grants)` (grants separated by spaces or commas) and
//! `declarePermission(name, description)`. Clients never reach any of this: nothing is sent to a client.

use std::cell::RefCell;
use std::rc::Rc;

use torque_engine::permissions::{ConnectionSubjects, Permissions, SubjectKey};
use torque_net::handshake::ConnId;
use torque_ts_vm::effects::Effects;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, NativeCall, Vm};

use super::NetGame;
use super::NetShared;

/// What a connection is held by (its permissions, its props): its auth identity, else the connection.
pub fn subject_key_of_conn<G: NetGame>(shared: &NetShared<G>, conn: ConnId) -> SubjectKey {
    let identity = shared.hs.connection(conn).and_then(|c| c.identity.as_ref());
    SubjectKey::of(identity, conn.0)
}

/// What a `GameConnection` object's permissions are kept under, if the host made it and accepted it.
pub fn subject_key<G: NetGame>(shared: &NetShared<G>, obj: ObjId) -> Option<SubjectKey> {
    let conn = shared.conns.get(&obj)?.conn?;
    Some(subject_key_of_conn(shared, conn))
}

/// The server's permissions and the key of the connection a method was called on.
fn target<G: NetGame>(
    vm: &Vm,
    shared: &Rc<RefCell<NetShared<G>>>,
    call: &NativeCall,
) -> Option<(Rc<Permissions>, SubjectKey)> {
    let perms = vm.extension::<Permissions>()?;
    let key = subject_key(&shared.borrow(), call.this_object?)?;
    Some((perms, key))
}

fn text(call: &NativeCall, at: usize) -> String {
    String::from_utf8_lossy(call.cstr(at)).into_owned()
}

/// The answer of a change: 1 when it changed something, 0 when it did not or was refused (the reason is printed).
fn changed(
    vm: &mut Vm,
    result: Result<bool, torque_engine::permissions::PermissionError>,
) -> Native {
    match result {
        Ok(did) => Native::Int(i32::from(did)),
        Err(e) => {
            vm.print(e.to_string().as_bytes());
            Native::Int(0)
        }
    }
}

/// Registers the permission natives on `vm`.
pub fn install<G: NetGame>(vm: &mut Vm, shared: &Rc<RefCell<NetShared<G>>>) {
    // Plugins read who a connection is through this, never through the script method below, which a game may redefine.
    {
        let shared = Rc::downgrade(shared);
        vm.set_extension(Rc::new(ConnectionSubjects::new(move |obj| {
            let shared = shared.upgrade()?;
            let shared = shared.try_borrow().ok()?;
            subject_key(&shared, obj)
        })));
    }
    type Method = fn(&mut Vm, &Permissions, &SubjectKey, &str) -> Native;
    let methods: [(&str, &str, Method); 6] = [
        (
            "addRole",
            "%client.addRole(name) - gives the player a role the game defined; 1 when it was new, 0 when held or refused",
            |vm, p, k, a| changed(vm, p.add_role(k, a)),
        ),
        (
            "removeRole",
            "%client.removeRole(name) - takes a role from the player; 1 when they held it",
            |_, p, k, a| Native::Int(i32::from(p.remove_role(k, a))),
        ),
        (
            "hasRole",
            "%client.hasRole(name) - whether the player holds the role",
            |_, p, k, a| Native::Bool(p.has_role(k, a)),
        ),
        (
            "grant",
            "%client.grant(grant) - gives the player a permission or a trailing-wildcard grant (props.*); 1 when it was new, 0 when held or refused",
            |vm, p, k, a| changed(vm, p.grant(k, a)),
        ),
        (
            "revoke",
            "%client.revoke(grant) - takes a direct grant from the player; 1 when they held it",
            |_, p, k, a| Native::Int(i32::from(p.revoke(k, a))),
        ),
        (
            "hasPermission",
            "%client.hasPermission(permission) - whether any grant of the player or of their roles covers the permission",
            |_, p, k, a| Native::Bool(p.holds(k, a)),
        ),
    ];
    for (name, usage, f) in methods {
        let shared = Rc::clone(shared);
        vm.register_in("GameConnection", name, 3, 3, usage, move |vm, call| {
            let Some((perms, key)) = target(vm, &shared, call) else {
                return Native::Int(0);
            };
            f(vm, &perms, &key, &text(call, 2))
        });
    }
    // What changes who may do what is a permissions effect: a script language is kept from it by default.
    for name in ["addRole", "removeRole", "grant", "revoke"] {
        vm.add_native_effects("GameConnection", name, Effects::PERMISSIONS);
    }
    vm.add_native_effects("", "definePermissionRole", Effects::PERMISSIONS);
    {
        let shared = Rc::clone(shared);
        vm.register_in(
            "GameConnection",
            "getRoles",
            2,
            2,
            "%client.getRoles() - the roles the player holds, as a word list",
            move |vm, call| {
                let roles = target(vm, &shared, call)
                    .map(|(perms, key)| perms.roles_of(&key).join(" "))
                    .unwrap_or_default();
                Native::Str(roles.into_bytes())
            },
        );
    }
    {
        let shared = Rc::clone(shared);
        vm.register_in(
            "GameConnection",
            "getIdentity",
            2,
            2,
            "%client.getIdentity() - who the server holds the player as: id:<hex> for a signed-in account (the raw account id, the same on every connection; do not show it to other players if the provider's ids are private), conn:<n> for an anonymous player (new on each connection); empty for no connection",
            move |_, call| {
                let key = call
                    .this_object
                    .and_then(|obj| subject_key(&shared.borrow(), obj));
                Native::Str(key.map(|k| k.to_string()).unwrap_or_default().into_bytes())
            },
        );
    }
    {
        let shared = Rc::clone(shared);
        vm.register_in(
            "GameConnection",
            "getIdentityName",
            2,
            2,
            "%client.getIdentityName() - the display name the auth provider gave the player, or empty (not unique, not stable)",
            move |_, call| {
                let name = call.this_object.and_then(|obj| {
                    let sh = shared.borrow();
                    let conn = sh.conns.get(&obj)?.conn?;
                    let identity = sh.hs.connection(conn)?.identity.as_ref()?;
                    identity.name().map(str::to_owned)
                });
                Native::Str(name.unwrap_or_default().into_bytes())
            },
        );
    }
    vm.register(
        "definePermissionRole",
        3,
        3,
        "definePermissionRole(name, grants) - defines (or redefines) a role as grants separated by spaces or commas; 1 when it was accepted",
        |vm, call| {
            let Some(perms) = vm.extension::<Permissions>() else {
                return Native::Int(0);
            };
            let result = perms.define_role(&text(call, 1), &text(call, 2)).map(|()| true);
            changed(vm, result)
        },
    );
    vm.register(
        "declarePermission",
        3,
        3,
        "declarePermission(name, description) - declares a permission the game's scripts check, so tools can list it; 1 when it was accepted",
        |vm, call| {
            let Some(perms) = vm.extension::<Permissions>() else {
                return Native::Int(0);
            };
            let result = perms.declare(&text(call, 1), &text(call, 2)).map(|()| true);
            changed(vm, result)
        },
    );
}
