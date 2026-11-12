//! The server's half: what its game scripts and Rust see of the realms of its players.
//!
//! A server tells each client which of its required mods have client scripts and what they ask for (the [`OFFER`] RPC, once a
//! client has an RPC link), receives each player's answer ([`STATUS`](crate::messages::STATUS)) and their messages ([`UP`](crate::messages::UP)), and sends values down
//! ([`DOWN`]). Everything a client sends is hostile input: a message is decoded from a length and chunks that must agree, within
//! its cap, held to the connection's rates, and handed on as a value. TorqueScript gets it as text (a table as JSON), so no
//! script text is ever built from it; the handlers are `clientScriptOnMessage(%client, %text)` and
//! `clientScriptOnStatus(%client, %state, %tiers)`, and `clientScriptSend(%client, %text)` sends a string down. In Rust,
//! [`ClientScriptsPlugin::on_message`](crate::ClientScriptsPlugin::on_message) takes the values.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use torque_client_script_format::Value;
use torque_net::handshake::ConnId;
use torque_server::net::commands::find_net_connection;
use torque_server::net::rpc::ServerRpc;
use torque_ts_vm::vm::{Native, Vm};

use crate::messages::{
    ByteBucket, DOWN, MAX_DOWN_BYTES, OFFER, OfferedMod, ScriptDown, ScriptOffer, ScriptStatus,
    Status, value_text,
};

/// A Rust handler of a player's message.
pub type MessageHandler = Rc<dyn Fn(&mut Vm, ConnId, &Value)>;

/// The most connections the byte buckets track at once.
const MAX_TRACKED: usize = 1_024;

/// What the server keeps of its players' realms.
#[derive(Default)]
pub struct ScriptServer {
    handlers: RefCell<Vec<MessageHandler>>,
    buckets: RefCell<HashMap<ConnId, ByteBucket>>,
    offered: RefCell<HashSet<ConnId>>,
    /// Messages dropped for going over the byte allowance.
    over_bytes: Cell<u64>,
    /// Messages refused as malformed or too large.
    refused: Cell<u64>,
    /// Messages handed on.
    delivered: Cell<u64>,
    /// What each player last said about the scripts: the word and the abilities named.
    statuses: RefCell<HashMap<ConnId, Status>>,
}

impl ScriptServer {
    pub(crate) fn add_handler(&self, handler: MessageHandler) {
        self.handlers.borrow_mut().push(handler);
    }

    /// Messages dropped for going over a connection's byte allowance.
    pub fn over_bytes(&self) -> u64 {
        self.over_bytes.get()
    }

    /// Messages refused as malformed or past their cap.
    pub fn refused(&self) -> u64 {
        self.refused.get()
    }

    /// Messages handed on to the server's handlers.
    pub fn delivered(&self) -> u64 {
        self.delivered.get()
    }

    /// What `conn`'s player last said about the scripts: `declined`, `unavailable` or `allowed`, with the abilities named.
    pub fn status_of(&self, conn: ConnId) -> Option<Status> {
        self.statuses.borrow().get(&conn).cloned()
    }

    /// Spends `bytes` of `conn`'s allowance at `tick`.
    fn allow(&self, conn: ConnId, tick: u64, bytes: usize) -> bool {
        let mut buckets = self.buckets.borrow_mut();
        // Past the cap the connection that spoke longest ago loses its place: forgetting every bucket would hand all of them
        // a full allowance, and a newcomer may not reset anyone's.
        if buckets.len() >= MAX_TRACKED
            && !buckets.contains_key(&conn)
            && let Some(stalest) = buckets
                .iter()
                .min_by_key(|(c, b)| (b.last_tick(), c.0))
                .map(|(c, _)| *c)
        {
            buckets.remove(&stalest);
        }
        buckets
            .entry(conn)
            .or_insert_with(|| ByteBucket::new(tick))
            .take(tick, bytes)
    }

    /// Takes in a player's message: its bytes counted against the connection's allowance, then handed to the handlers.
    pub(crate) fn receive(
        &self,
        vm: &mut Vm,
        conn: ConnId,
        tick: u64,
        up: &crate::messages::ScriptUp,
    ) {
        let value = match up.value() {
            Ok(value) => value,
            Err(_) => {
                self.refused.set(self.refused.get().saturating_add(1));
                return;
            }
        };
        if !self.allow(conn, tick, usize::from(up.len)) {
            self.over_bytes.set(self.over_bytes.get().saturating_add(1));
            return;
        }
        self.delivered.set(self.delivered.get().saturating_add(1));
        let handlers: Vec<MessageHandler> = self.handlers.borrow().clone();
        for handler in handlers {
            handler(vm, conn, &value);
        }
        call_script(vm, conn, "clientScriptOnMessage", &[value_text(&value)]);
    }

    /// Takes in a player's answer.
    pub(crate) fn status(&self, vm: &mut Vm, conn: ConnId, status: &ScriptStatus) {
        let Some(status) = status.status() else {
            self.refused.set(self.refused.get().saturating_add(1));
            return;
        };
        let names: Vec<&str> = match &status {
            Status::Allowed(tiers) => tiers.iter().map(|t| t.name()).collect(),
            _ => Vec::new(),
        };
        let args = [status.word().to_string(), names.join(",")];
        {
            let mut statuses = self.statuses.borrow_mut();
            if statuses.len() >= MAX_TRACKED && !statuses.contains_key(&conn) {
                statuses.clear();
            }
            statuses.insert(conn, status);
        }
        call_script(vm, conn, "clientScriptOnStatus", &args);
    }
}

/// Calls the server script function `function` for `conn`'s player with `args`, when the game defines it. The player's
/// words are arguments, never script text.
fn call_script(vm: &mut Vm, conn: ConnId, function: &str, args: &[String]) {
    let Some(object) = vm
        .extension::<ServerRpc>()
        .and_then(|rpc| rpc.object_of(conn))
    else {
        return;
    };
    if vm.global_function_origin(function.as_bytes()).is_none() {
        return;
    }
    let mut argv = vec![object.to_string().into_bytes()];
    argv.extend(args.iter().map(|a| a.clone().into_bytes()));
    if let Err(error) = vm.invoke(None, function.as_bytes(), &argv) {
        vm.warn(format!("{function}: {error:?}"));
    }
}

/// Sends `value` down to `conn`'s realm.
///
/// # Errors
///
/// Why it was not queued: past [`MAX_DOWN_BYTES`], no RPC link for the client, or the link's own refusals.
pub fn send_down(vm: &Vm, conn: ConnId, value: &Value) -> Result<(), String> {
    let message =
        ScriptDown::of(value).map_err(|_| format!("the value is over {MAX_DOWN_BYTES} bytes"))?;
    let rpc = vm
        .extension::<ServerRpc>()
        .ok_or_else(|| "no RPCs".to_string())?;
    rpc.with_link(conn, |link| {
        let handle = link
            .rpc::<ScriptDown, ()>(DOWN)
            .map_err(|e| e.to_string())?;
        link.notify(&handle, message).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("that client has no RPC link".to_string()))
}

/// Tells `conn` about the server's client scripts: the mods required with their entries and abilities.
///
/// # Errors
///
/// Why it was not queued.
pub fn send_offer(vm: &Vm, conn: ConnId, mods: &[OfferedMod]) -> Result<(), String> {
    let rpc = vm
        .extension::<ServerRpc>()
        .ok_or_else(|| "no RPCs".to_string())?;
    let message = ScriptOffer::of(mods);
    rpc.with_link(conn, |link| {
        let handle = link
            .rpc::<ScriptOffer, ()>(OFFER)
            .map_err(|e| e.to_string())?;
        link.notify(&handle, message).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|| Err("that client has no RPC link".to_string()))
}

/// The server's client scripts as an offer: the required mods whose manifests ask for scripts.
pub fn current_offer(vm: &Vm) -> Vec<OfferedMod> {
    torque_mods::Mods::of(vm)
        .map(|mods| {
            mods.client_script_offers()
                .into_iter()
                .map(|o| OfferedMod {
                    id: o.mod_id,
                    hash: o.hash,
                    entry: o.entry,
                    tiers: o.tiers,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Once a tick: every client that has an RPC link and has not been told is sent the offer, and a client that left is forgotten.
pub(crate) fn offer_new_clients(vm: &mut Vm) {
    let (Some(rpc), Some(server)) = (vm.extension::<ServerRpc>(), vm.extension::<ScriptServer>())
    else {
        return;
    };
    let present = rpc.connections();
    server.offered.borrow_mut().retain(|c| present.contains(c));
    server
        .buckets
        .borrow_mut()
        .retain(|c, _| present.contains(c));
    server
        .statuses
        .borrow_mut()
        .retain(|c, _| present.contains(c));
    let offer = current_offer(vm);
    if offer.is_empty() {
        return;
    }
    for conn in present {
        if server.offered.borrow().contains(&conn) {
            continue;
        }
        if send_offer(vm, conn, &offer).is_ok() {
            server.offered.borrow_mut().insert(conn);
        }
    }
}

/// Registers the server's TorqueScript functions: `clientScriptSend(client, text)` and `clientScriptOffer(client)`.
pub(crate) fn register_natives(vm: &mut Vm) {
    vm.register(
        "clientScriptSend",
        3,
        3,
        "clientScriptSend(client, text) - sends a client's server-script realm a string; answers the reason it was not queued, or nothing",
        |vm, call| {
            let Some(conn) = conn_of(vm, call.cstr(1)) else {
                return Native::Str(b"no such client".to_vec());
            };
            let value = Value::String(call.cstr(2).to_vec());
            Native::Str(match send_down(vm, conn, &value) {
                Ok(()) => Vec::new(),
                Err(why) => why.into_bytes(),
            })
        },
    );
    vm.register(
        "clientScriptOffer",
        2,
        2,
        "clientScriptOffer(client) - tells a client about this server's client scripts again; answers the reason it was not queued, or nothing",
        |vm, call| {
            let Some(conn) = conn_of(vm, call.cstr(1)) else {
                return Native::Str(b"no such client".to_vec());
            };
            let offer = current_offer(vm);
            Native::Str(match send_offer(vm, conn, &offer) {
                Ok(()) => Vec::new(),
                Err(why) => why.into_bytes(),
            })
        },
    );
}

fn conn_of(vm: &Vm, client: &[u8]) -> Option<ConnId> {
    let object = find_net_connection(vm, client)?;
    vm.extension::<ServerRpc>()?.conn_of(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_the_tracked_connections_the_stalest_goes_and_nobody_gets_a_fresh_allowance() {
        let server = ScriptServer::default();
        for n in 0..MAX_TRACKED as u32 {
            assert!(server.allow(ConnId(n), u64::from(n), 1));
        }
        // The newest connection spends its whole allowance.
        let busy = ConnId(MAX_TRACKED as u32 - 1);
        let tick = MAX_TRACKED as u64 - 1;
        assert!(server.allow(busy, tick, 60_000));
        assert!(!server.allow(busy, tick, 60_000));
        // A thousand newcomers arrive: the table stays within its cap and the busy connection stays limited.
        for n in 0..1_000u32 {
            assert!(server.allow(ConnId(10_000 + n), tick + 1, 1));
            assert!(server.buckets.borrow().len() <= MAX_TRACKED);
        }
        assert!(
            !server.allow(busy, tick + 1, 60_000),
            "its bucket was not reset"
        );
        // The ones that went were the stalest.
        let buckets = server.buckets.borrow();
        assert!(!buckets.contains_key(&ConnId(0)));
        assert!(buckets.contains_key(&busy));
    }
}
